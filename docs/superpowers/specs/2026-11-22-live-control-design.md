# Live control: an AI agent builds the setup while you watch

Status: draft for approval, 2026-11-22.

## Why

The user wants an AI agent (for example Claude Code in another window) to build an optical setup inside
the LabSetup window that is already open, while they watch it happen and can step in at any time.

Today nothing outside the app can change an open project. `labsetup-cli` only reads saved files
(`info`, `list`, `bom`, `validate`, `catalog`), and the app has no file watcher or IPC.

## Goals

- While LabSetup runs, an agent can **read** the open project and **edit** it. Every edit shows in the
  window immediately, exactly like one made by hand: 3D view, scene tree, inspector, beam trace and autosave.
- The agent speaks **lab terms**: put a part in breadboard hole C14, stack post holder + post + mount + optic
  on it at a beam height, mate one part's connector to another's. Raw coordinates are the fallback.
- The agent can **assign constraints** (Mate, Flush, Concentric, Angle) between features of two parts,
  the same constraints the Constrain Components panel makes.
- The agent can **check its own work**: list parts and poses, list a part's features, read the hole grid,
  read the beam trace, and take a PNG snapshot of the view.
- The user stays in charge: each agent step is one labelled undo entry ("Agent: place KM100"), the user can
  keep editing during a session, and a badge shows when an agent is connected.
- Phase 1 is a CLI (`labsetup-cli live …`) that anyone can type, too. Phase 2 is an MCP server over the
  same protocol.

## Non-goals

- Network access or remote control from another machine. The channel is local and per user only.
- Several people editing together, or syncing between computers.
- Joints (M9 `Attach::Joint`), room and wall editing, platform creation or reshaping, file open/save/new,
  settings, catalog import or update. These can be added later as protocol commands. In phase 1 the agent
  works on an existing project with its platforms already in place.
- Automatic beam aiming ("turn this mirror so the beam hits that lens"). The agent sets rotations itself
  and checks the result with `beams`.
- A chat panel or agent UI inside LabSetup. The agent runs in its own window.
- Linux or macOS. The transport is a Windows named pipe, which is what the app ships on today.

## Anti-goals (failure even if it "works")

- An agent edit that skips the undo history, the checks or the view update, so the window shows something
  other than the document, or Ctrl+Z can't undo it.
- The UI freezes while a request is read or answered. Pipe I/O never runs on the UI thread.
- A network-capable crate in `labsetup.exe` (`scripts/check-offline.ps1` must still pass).
- Any process other than the user's own can connect: another user on the PC, or a remote client.
- An edit lands in a different project than the agent read. For example the user opens another file
  mid-session, and the agent's next "move node 12" changes the wrong part.
- A half-applied multi-part step. A stack of four parts is one command that lands completely or not at all.

## Approaches considered

### How the agent says where things go

| | Approach | Trade-off | Outcome |
|---|---|---|---|
| A | Raw coordinates: "add part X at x, y, z with rotation r" | Least to build (the existing `Command`s plus a text form); the agent does all the geometry and gets small misalignments it only finds afterwards | Kept as the **fallback** |
| **B** | **Lab terms: holes, stacks, connector mates, feature constraints** | **More to build; reuses the hole grid, `stack_chain`, `lab_mates` and feature detection, so parts land seated correctly and steps read like bench work** | **Chosen by the user** |

### How the agent connects

The app listens on a local named pipe in every option. What differs is the client.

| | Approach | Trade-off | Outcome |
|---|---|---|---|
| 1 | CLI only (`labsetup-cli live …`) | Least to build; works with any agent and by hand; the agent can't see images unless it opens a PNG itself | Folded into 3 |
| 2 | MCP server only | Typed tools and inline snapshot images; one more binary; useful only to MCP-capable agents | Folded into 3 |
| **3** | **Both: pipe protocol + CLI now, a thin MCP server later** | **Phase 1 is testable by hand; phase 2 is small because all logic lives in the app** | **Chosen by the user** |

## Design

### Shape

```
agent ──shell──> labsetup-cli live <cmd>  ─┐
agent ──MCP────> labsetup-mcp (phase 2)   ─┼─ JSON line ─> \\.\pipe\labsetup-<user>-<pid>
                                           │                    │ listener thread (lab-app)
                                           │                    ▼ slint::invoke_from_event_loop
                                           │               live::handle(request)   (UI thread)
                                           │                    │ reads: answer from AppState
                                           │                    │ writes: live::resolve → Command
                                           │                    ▼ controller::execute (same as a click)
                                           └──── JSON line <── response
```

- **Listener** (new `crates/lab-app/src/live.rs`, wired in `main.rs` next to `bridge::listen_to_io`):
  a background thread creates the pipe with `windows-sys` (already a dependency). It keeps several pipe
  instances open, so a CLI call still gets through while the phase 2 MCP server holds its connection.
  Every connection feeds the same in-order queue to the UI thread through `slint::invoke_from_event_loop`.
  The answer comes back over a channel, and the connection's thread writes it out. *(Designer's call,
  from the builder check.)*
- **Resolve, then execute.** Each write request is first turned into one `lab_core::Command` by a pure
  function, `live::resolve(&Request, &Document, &Catalog, &dyn FeatureLookup) -> Result<Resolved, LiveError>`,
  where `Resolved = { command, label, select }`. `FeatureLookup` gives a node's features (see Features).
  It is cached in the app and stubbed in tests. `resolve` holds all the lab-terms logic and has no UI, so it
  is unit-tested on plain documents. The result then goes through the existing
  `controller::execute(ui, app, models, "live", command, label, select, vec![])`. Undo, view sync,
  selection, refusal messages and autosave are therefore identical to a hand edit. `execute` returns
  only a `bool` and writes the refusal text into the inspector. The `refused` error has to carry that same
  text, so the builder returns it from `execute`, or validates it with `Command::apply` on a clone first.
- **Multi-part steps** (a stack, a place-and-constrain) become one `Command::PlaceAssembly` or
  `Command::Batch`, so they are one undo entry and land all-or-nothing.

### Transport and security

- Pipe name `\\.\pipe\labsetup-<username>-<pid>`. One pipe per running LabSetup process.
- Created with `PIPE_REJECT_REMOTE_CLIENTS`. The security descriptor grants access to the current user's SID
  only (not the default DACL, which gives Everyone read access).
- **Off by default.** It's turned on by a setting, "Allow AI agents to control LabSetup", in the Settings
  sheet, saved in `Settings`. While it's off, no pipe exists. *(Designer's call: a local same-user process
  can already edit project files, but an always-open control channel should still be an explicit choice.)*
- **Finding the app:** `labsetup-cli live` lists `\\.\pipe\` entries that match `labsetup-<username>-*`.
  If there's exactly one, it uses it. If there are several, it fails with a list of pids and window titles,
  and `--pid <n>` picks one. If there are none, it says live control is off or LabSetup isn't running.
- **Framing:** UTF-8 JSON, one object per line, each way. One request per connection: the CLI connects,
  sends, reads, exits. A persistent connection is allowed (the MCP server keeps one open), and requests on
  it are answered in order.

### Protocol

Request: `{"v":1, "cmd":"<name>", "args":{…}, "doc":"<token>"?}`

Response: `{"ok":true, "doc":"<token>", "result":{…}}` or
`{"ok":false, "doc":"<token>", "error":{"code":"<code>", "message":"<what the user would see>"}}`

- **Units on the wire:** millimetres and degrees, always. Positions and rotations are in the parent's
  frame, as stored in the document. Rotations are given as ZYX Euler degrees, matching `labsetup-cli list`.
- **Document token** (`doc`): changes whenever the open document is replaced (new, open, recover, revert).
  It doesn't change on edits. Every response carries it. If a write request includes `doc` and it doesn't
  match, the request is refused with `doc_changed`. The CLI sends the token it was given automatically
  when `--doc` is passed; agents are told to pass it. This prevents the wrong-project anti-goal.
- **Error codes:** `bad_request` (malformed, unknown command or argument), `not_found` (node, platform,
  part, hole, connector or feature), `refused` (the `CommandError` text, the same as the inspector shows),
  `busy` (see Concurrency), `doc_changed`, `disabled`.

### Commands, phase 1

Reads:

| cmd | args | result |
|---|---|---|
| `status` | | app version, project title, file path, dirty flag, doc token, undo label at the top of the history |
| `list` | `platform?` | nodes: id, name, part, vendor/part number, parent, attach kind, position, rotation, locked |
| `platforms` | | id, name, kind, top height, outline bounds, whether it has a hole grid |
| `holes` | `platform` | grid pitch, first/last label, thread size, and occupied holes with the node in each |
| `part` | `id` (catalog id) | name, vendor, part number, connectors (name, kind, frame) |
| `catalog` | `search`, `limit?` | catalog id, vendor, part number, name |
| `features` | `node` | the node's named connectors plus detected features (see Features), each with an id and a description |
| `beams` | | traced segments: start, end, which node each one hits, beam radius there |
| `snapshot` | `path?`, `width?`, `height?` | writes a PNG of the current view through `export_png::render_to`; returns the path |

Writes (each is one undo entry, labelled `Agent: …`):

| cmd | args | does |
|---|---|---|
| `place` | `part`, then one of: `{platform, hole}` / `{on: node, conn, parent_conn, slide?, twist?}` / `{parent?, position, rotation}` (fallback A); `name?`, `rotate_z?` | adds one part, bolted to a hole (`Attach::Hole`), mated to a connector of another node (`Attach::Mate`), or free |
| `stack` | `platform`, `hole`, `holder`, `post`, `mount`, `base?`, `optic?`, then one of `axis_height` / `slide`; `rotate_z?` | builds the stack in one step with `lab_viewport::stack_chain`, the same chain the library drag uses |
| `constrain` | `node`, `to` (node, or platform for its top face), list of `{child, parent, kind?, offset?, angle?, flip?, lock_rotation?}` | sets `Attach::Constrained` through `lab_mates::constraints::constrain`; `kind` left out means `default_kind`, like the panel |
| `move` | `node`, `position?`, `rotation?` (fallback A) | `Command::SetPlacement`; refused for nodes that are attached, as in the UI |
| `set_params` | `node`, `params` | `Command::SetParams` (laser wavelength, generator sizes) |
| `rename` | `node`, `name` | `Command::Rename` |
| `note` | `node`, `text` | `Command::SetNote` |
| `remove` | `nodes` | `Command::RemoveNodes` |
| `undo` / `redo` | | the same as Ctrl+Z / Ctrl+Y, whoever made the step |

Write results return the ids of the nodes added or changed, their final poses, and the new top undo label.

**Hole names** are the labels the UI shows (`C14`: row letter plus column number, from
`HoleSet::label`), so the agent and the user talk about the same hole. `[col,row]` lattice indices are
accepted too.

**Beam height:** `stack` takes `axis_height` (mm from the platform top to the optic's axis) and works out
the post slide from the connector frames. Or it takes `slide` directly. It's refused if the height can't be
reached with that post.

### Features (for constraints)

The user picks faces by clicking. The agent needs names instead. `features` returns, for one node:

- its **named connectors** from the catalog (`bottom`, `bore`, `top`, `optic_seat`, …), with id = the
  connector name, and
- the **detected features** of its mesh: planes and axes. Imported parts already store these in
  `PartDef.features` (made by `lab_import::features::detect` at import). Built-in parts store none, so
  `detect` runs on the mesh `constrain::part_mesh` builds, which depends on the node's params. They get ids
  `f1`, `f2`, … in a stable order, and a description such as `"plane, normal +Z, at z=0 (bottom face)"` or
  `"hole axis Ø6.0 along +Z, opening at z=12.5"`. Everything is in the part frame.

`constrain` accepts either kind of id for `child` and `parent`. Connectors become `Feature::Plane` or
`Feature::Axis` from their frame. For `to: platform`, the parent features are `top` (the top plane) and
`hole:C14` (that hole's axis).

Feature ids are stable while the part's geometry doesn't change. The order is fixed by sorting
`detect`'s output (designer's call; the exact key is left to the builder).

### Watching

- **Badge:** a small "Agent connected" pill in the title bar area while a client has connected in the last
  10 s, showing the label of the agent's last step. Clicking it opens the setting to switch live control off.
- **Selection follows the agent:** the part an agent step adds or changes becomes selected, as with a hand
  edit, so the user sees which part it is. The camera does not move.
- **Pacing:** write requests are applied no faster than one every 300 ms. Further requests wait in a
  queue, and their responses come once applied. *(Designer's call, so a fast agent doesn't just make the
  setup appear all at once. The delay could become a setting later.)* Reads are never delayed.

### Concurrency with the user

- Agent writes and the user's edits go through one history on one thread, so they never interleave
  half-way. If the user deleted a node the agent then refers to, the agent gets `not_found`.
- While a gesture or modal state is in progress (a drag in the view, the Constrain Components panel open,
  a sheet or dialog up, the Place wizard), writes return `busy`, and the agent retries. Reads still work.
  *(The builder finds the existing flags in `AppState` that mark these states.)*
- Agent steps take part in autosave and the dirty flag as normal. The agent can't save. The user saves.

### CLI (phase 1 client)

`labsetup-cli live <cmd> [args]`, in the existing binary. Each command maps one-to-one onto the protocol.
Arguments are flags (`--platform 1 --hole C14`) or `--json '<args object>'` for anything nested
(constraint lists). Output is always the JSON response on stdout, with exit code 0 for `ok`, 1 for an error
response, and 2 for usage or connection failures, matching the existing CLI's convention.
`labsetup-cli live --help` lists every command with an example, written for an agent to read.

The CLI doesn't link Slint or Bevy, as now. Pipe client code uses `windows-sys` or `std::fs::OpenOptions`
on the pipe path.

### MCP server (phase 2)

A small `labsetup-mcp` binary (stdio MCP) keeps one pipe connection open and exposes one tool per protocol
command with a JSON schema for its args. `snapshot` returns the PNG as an image content block, so the
agent sees the view directly. It holds no logic of its own. Building it is a separate plan after phase 1
ships.

## Testing

- `live::resolve` unit tests on documents built in code (`lab_core::sample::demo_document` plus small
  fixtures): hole placement lands on the right hole position, `stack` matches what `stack_chain` gives the
  library drag, `constrain` with connector ids and with detected-feature ids, the `axis_height` to slide
  conversion, and refusals (unknown hole, occupied hole if the UI refuses it, attached node `move`).
- Protocol parse and serialise tests: every command, the error shapes, `doc_changed`.
- An end-to-end test that starts the listener thread against a stub handler and talks to it over a real
  pipe, including the remote-reject and same-user security descriptor.
- Manual check: with the app open, build a two-mirror beam path entirely from `labsetup-cli live`, watch it
  appear, Ctrl+Z every step, and confirm `beams` reports the path.

## Left to the builder

- Module layout inside `lab-app`, the exact Rust types for requests and responses (serde enums are
  expected), and how the UI-thread answer gets back to the pipe thread.
- How detected features are sorted into stable ids, and the exact wording of their descriptions.
- The badge's look, following `theme.slint` and the existing title-bar elements.
- Which existing `AppState` flags define `busy`.
- Whether `holes` lists free holes as well as occupied ones, if that's cheap.
