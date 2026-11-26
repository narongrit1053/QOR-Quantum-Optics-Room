# Agent connect: any coding-agent CLI finds and drives LabSetup

## Intent

Today an agent can drive LabSetup only if the user points it at `labsetup-cli.exe` (it sits in the install
folder, which is not on PATH) and explains the `live` commands, or opens the terminal in that folder. Every
session starts from zero: the agent reads `live --help`, works out the order of calls, and guesses.

The user wants anyone who installs LabSetup to be able to open **any** coding-agent CLI (Claude Code,
Codex, Gemini CLI, Antigravity, Cursor, Copilot CLI, opencode, …) in **any** folder, and have the agent
already know LabSetup exists, find the running app, and control it directly. This is the "thin MCP server
later" (phase 2) of `2026-11-22-live-control-design.md`.

## Goals

- `labsetup-cli mcp`: an MCP server over stdio that exposes every `live` command as a typed tool and talks
  to the running app over the existing pipe. No app logic moves; the app stays the single source of truth.
- The server's MCP `instructions` carry the working method, so an agent connecting for the first time
  knows how to work without a separate skill: read `status` first and pass its doc token on writes; read
  `holes` / `features` before `place` / `constrain`; check with `snapshot`; units are mm and degrees;
  each write is one undo step.
- `snapshot` returns the image inline (MCP image content), not just a file path.
- One-step registration in every agent CLI found on the PC:
  - a **Connect AI agents…** button in Settings, next to *Allow AI agents to control LabSetup*;
  - the same thing headless: `labsetup-cli agents install | uninstall | status`.
- The installer adds the install folder to PATH, so `labsetup-cli` works from any shell as a
  fallback for agents that don't speak MCP.

## Non-goals

- No change to the pipe protocol, the resolver or the permission model. *Allow AI agents* stays off by
  default and still gates everything.
- No skill files or always-loaded instruction-file edits (AGENTS.md, GEMINI.md, CLAUDE.md). The MCP
  instructions replace them. A longer Claude Code skill can come later if the instructions prove too thin.
- No network transport (HTTP/SSE). Local stdio only.
- No auto-launching LabSetup from the MCP server.

## Anti-goals (a failure even if it works)

- Installing overwrites or corrupts a user's existing agent config (their other MCP servers, comments,
  settings). Configs are merged, never replaced, and backed up first.
- The agent gets a vague failure when the app is closed or the setting is off. Every tool call in that
  state answers with the same plain message the CLI gives today ("LabSetup isn't running, or 'Allow AI
  agents' is off in Settings.") so the agent can tell the user what to do.
- Tool list and `live --help` drift apart: a command exists in one and not the other.
- A new MCP or TOML dependency pulled in for a few hundred lines of JSON-RPC.

## Constraints

- Windows only (the pipe is a Windows named pipe). Paths below are Windows.
- Rust, in `crates/lab-app`; `serde_json` is already there. Builds use `--offline` and the shared target
  dir; no `cargo update`, no new dependency without the user's say-so.
- The registered command must be the absolute path of the `labsetup-cli.exe` that ran the install
  (`std::env::current_exe`), so it works without PATH. A moved portable copy needs a re-install; `status`
  reports a stale path.

## Approaches

| | Approach | Trade-off | Outcome |
|---|---|---|---|
| **A** | **MCP server in `labsetup-cli` + "Connect AI agents" registration in every agent CLI's global config** | **One piece covers every MCP-capable agent; tool descriptions and server instructions carry the know-how; inline snapshots. Cost: the server plus a small config writer per agent CLI** | **Chosen by the user** |
| B | CLI on PATH + a skill/instruction file installed into each agent | Least to build; but only some agents have on-demand skills, others would load the text in every session; no inline images | Rejected; PATH part kept as fallback |
| C | A + a Claude Code skill with long worked examples | Extra file to keep in sync; only Claude Code benefits | Deferred until A's instructions prove too thin |

## Design

### MCP server (`labsetup-cli mcp`)

- Minimal JSON-RPC 2.0 over stdin/stdout, newline-delimited, hand-written with `serde_json`: `initialize`
  (returns server info, `tools` capability and the `instructions` text), `notifications/initialized`,
  `tools/list`, `tools/call`, `ping`. Unknown methods get the standard "method not found" error.
- **One tool per live command**, named `labsetup_<cmd>` (e.g. `labsetup_place_bundle`), each with a
  description and a JSON Schema for its args matching the `ReadRequest` / `WriteRequest` / `AppRequest`
  variants. Every tool also takes an optional `pid` for when several LabSetup windows run (same as
  `--pid`); writes take `doc`.
- `tools/call` builds the same request line the CLI builds and sends it with `live::pipe::call` (reuse the
  CLI's existing connect/pick-instance code, don't copy it), with the CLI's existing timeouts (2 s connect,
  30 s reply). Slow work is already asynchronous in the app (`book-import` returns at once and is polled
  with `book-list`); the tool descriptions say so.
- The server keeps no state between calls: it does not remember `pid` or the doc token. *(Designer's
  call: the doc token is the guard against editing a project the agent didn't read; caching it in the
  server would defeat that.)* The app's reply goes back as text content;
  an error reply sets `isError: true`.
- `labsetup_snapshot`: if the agent gives no path, the server picks a temp file, then reads the PNG and
  returns it as image content (plus the path as text).
- **One table drives both** `live --help` and `tools/list` (name, summary, example, schema), so they cannot
  drift. A unit test deserializes each tool's example args into the matching request enum, and fails when
  a request variant has no tool.
- `instructions`: the working method above, the lab vocabulary (hole names like C14, stacks, mate kinds),
  and "if a call says LabSetup isn't running, ask the user to open it and turn on *Allow AI agents*".

### Registration (`labsetup-cli agents …` and the Settings button)

- A table of known agent CLIs: name, how to detect it (its config dir exists, or its executable is on
  PATH), config file path, format (JSON or TOML) and where the server entry goes. Starting list, each to be
  checked against the tool's current documentation when built:

  | Agent | Config | Entry |
  |---|---|---|
  | Claude Code | `~/.claude.json` (user scope) | `mcpServers.labsetup` |
  | Codex CLI | `~/.codex/config.toml` | `[mcp_servers.labsetup]` |
  | Gemini CLI | `~/.gemini/settings.json` | `mcpServers.labsetup` |
  | Antigravity | its user MCP config | `mcpServers.labsetup` |
  | Cursor | `~/.cursor/mcp.json` | `mcpServers.labsetup` |
  | GitHub Copilot CLI | `~/.copilot/mcp-config.json` | `mcpServers.labsetup` |
  | opencode | `~/.config/opencode/opencode.json` | `mcp.labsetup` (`type: local`, command as array) |
  | Windsurf | `~/.codeium/windsurf/mcp_config.json` | `mcpServers.labsetup` |
  | VS Code (Copilot agent) | `%APPDATA%\Code\User\mcp.json` | `servers.labsetup` |

  The entry is always `command = <abs path to labsetup-cli.exe>`, `args = ["mcp"]`.
- Always edit the config file directly, even where an agent has its own add command (e.g.
  `claude mcp add`). *(Designer's call: one code path gives backups, added/updated/already-set reporting,
  `status` and `uninstall` the same way for every agent.)*
- The registration code lives in the `lab_app` library and takes the path of the `labsetup-cli.exe` to
  register. The CLI passes `current_exe`; the Settings button passes the `labsetup-cli.exe` next to
  `labsetup.exe` and refuses with a clear message if it isn't there. *(Designer's call: `current_exe`
  inside the GUI is `labsetup.exe`, which would start the window instead of the server.)*
- File edits: read, back up to `<file>.labsetup-bak`, insert or replace only the `labsetup` entry, write
  atomically (`lab_file::atomic_write`). A file that doesn't parse is left alone and reported, never
  rewritten. TOML: Codex's file is edited as text (append or replace the `[mcp_servers.labsetup]` table),
  so no TOML crate is needed and comments survive.
- `install` writes to every detected agent and prints one line per agent: added / updated / already set /
  skipped (not found) / failed (why). `uninstall` removes only the `labsetup` entry. `status` lists each
  agent and whether its entry points at this exe.
- For anything not in the table, `install` also prints a copyable JSON snippet.
- Settings: **Connect AI agents…** runs the same install and shows the per-agent result in a small
  dialog. It is enabled regardless of *Allow AI agents*, but the dialog says that control also needs that
  switch on.

### Installer

- `installer/labsetup.wxs`: add the install folder to the **system** PATH (WiX `Environment
  System="yes"`, removed on uninstall). *(Designer's call: the MSI is per-machine in Program Files, so a
  system entry matches it and is cleaned up for every user; a user entry would be left behind when another
  user uninstalls.)* The MSI does **not** register agents; the user does that from Settings.

## Left to the builder

- Exact JSON Schemas per tool, description wording, and the text of `instructions` (keep it short; it is
  sent at every connect).
- The MCP protocol version string to advertise, and handling of protocol-version negotiation.
- Confirming each agent's config path and entry shape against its current docs, and dropping any that has
  no user-level MCP config.
- Layout of the Settings dialog, following the existing Settings window.
- Tests: protocol round trip over in-memory stdin/stdout; config merge on sample files (empty, existing
  servers, existing `labsetup` entry, unparseable); the tool/request coverage test above.
