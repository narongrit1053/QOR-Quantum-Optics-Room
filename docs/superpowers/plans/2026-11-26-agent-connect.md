# Agent Connect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any coding-agent CLI on the PC finds the running LabSetup and drives it through an MCP server, registered in one step from Settings or `labsetup-cli agents install`.

**Architecture:** One tool table in `lab_app::live::tools` drives both `live --help` and the MCP tool list. `lab_app::live::mcp` is a hand-written stdio JSON-RPC loop that turns tool calls into the existing pipe request lines. `lab_app::agents` merges a `labsetup` server entry into each agent CLI's config file. The CLI and the Settings window are thin callers.

**Tech Stack:** Rust (`crates/lab-app`), `serde_json`, existing `live::pipe`, Slint for the Settings button, WiX for PATH.

**Spec:** `docs/superpowers/specs/2026-11-26-agent-connect-design.md`

## Global Constraints

- Windows only; the pipe code is `#[cfg(windows)]`. Keep new pipe-touching code behind the same cfg.
- No new dependencies and no feature changes to existing ones; no `cargo update`. Builds use `--offline`.
- Build env (Git Bash): `export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH CARGO_TARGET_DIR=D:/Try/LabSetup/target`. Bevy/Slint links take minutes; use long timeouts. "Blocking waiting for file lock" means wait.
- Server entry name is `labsetup`; command is the absolute path of `labsetup-cli.exe`, args `["mcp"]`.
- Timeouts as the CLI today: 2 s connect, 30 s reply (`pipe::call`).
- Not-running message, verbatim: `LabSetup isn't running, or 'Allow AI agents' is off in Settings.`
- Backups go to `<config file>.labsetup-bak`; writes use `lab_file::atomic_write`. A config file that doesn't parse is never written.
- The MCP server keeps no state between calls (no remembered `pid` or doc token).
- After code changes run `graphify update .` (project CLAUDE.md).

## Review Focus

1. **Agent config with JSON comments (JSONC, e.g. VS Code `mcp.json`)** → reported as "failed: can't read …, left unchanged", file byte-identical. Test in Task 3.
2. **Config file whose `mcpServers` already holds the user's other servers** → those stay, with their values unchanged; only `labsetup` is added. Test in Task 3.
3. **LabSetup closed or switch off when the agent calls a tool** → tool result `isError: true` with the verbatim not-running message, and the server keeps running for the next call. Test in Task 2.
4. **Several LabSetup windows and no `pid`** → `isError` listing the pids, not a pick of one at random. Test in Task 2 (via the shared `pick` error text).
5. **Re-running install after moving a portable copy** → entry rewritten to the new path, reported `updated`; `status` shows the old path as stale before that. Test in Task 3.

The rewritten JSON is re-serialized by `serde_json` without `preserve_order`, so key order in the user's file may change. That is accepted: changing serde_json features is forbidden. Mark it in the code with `// shortcut: key order not kept (serde_json without preserve_order); switch to an order-keeping writer if users complain`.

---

### Task 1: One tool table for help and tools; shared request building

**Files:**
- Create: `crates/lab-app/src/live/tools.rs`
- Modify: `crates/lab-app/src/live/mod.rs` (add `pub mod tools;`)
- Modify: `crates/lab-app/src/live/protocol.rs` (add `Request::cmd_name`)
- Modify: `crates/lab-app/src/live/pipe.rs` (add `pick`)
- Modify: `crates/lab-app/src/bin/labsetup-cli.rs` (`LIVE_HELP` from the table; `parse_live` and `run_live` use the shared helpers)

**Interfaces:**
- Produces:
  - `pub enum Group { Read, Write, App }`
  - `pub struct Tool { pub cmd: &'static str /* CLI spelling, e.g. "place-bundle" */, pub group: Group, pub usage: &'static str /* the flags column of today's help */, pub summary: &'static str, pub example: &'static str /* full CLI example line */, pub example_args: fn() -> serde_json::Value, pub schema: fn() -> serde_json::Value /* JSON Schema object for args, without pid/doc */ }`
  - `pub fn tools() -> &'static [Tool]`
  - `pub fn find(cmd: &str) -> Option<&'static Tool>` (accepts `place-bundle` or `place_bundle`)
  - `pub fn request_line(cmd: &str, args: serde_json::Map<String, Value>, doc: Option<&str>) -> String` → `{"v":1,"cmd":"<snake>","args":{…},"doc":…}` exactly as `parse_live` builds it today
  - `pub fn help_text() -> String` → the full `live --help` text: today's preamble paragraph, then Reads / Writes / App sections generated from the table, today's closing notes kept
  - `impl Request { pub fn cmd_name(&self) -> &'static str }` — exhaustive `match` over every variant, snake_case wire name. Adding a variant without a name fails to compile.
  - `pub fn pick(pid: Option<u32>) -> Result<String, String>` in `pipe.rs` — the pipe-choosing logic moved out of `run_live`, same error texts.

- [ ] **Step 1: Write the failing tests** in `tools.rs`:

```rust
#[test]
fn every_tool_example_parses_to_its_own_command() {
    for t in tools() {
        let line = request_line(t.cmd, (t.example_args)().as_object().unwrap().clone(), Some("T"));
        let env = protocol::parse_line(&line).unwrap_or_else(|e| panic!("{}: {e:?}", t.cmd));
        assert_eq!(env.request.cmd_name(), t.cmd.replace('-', "_"));
    }
}

#[test]
fn table_covers_every_command() {
    let names: BTreeSet<_> = tools().iter().map(|t| t.cmd.replace('-', "_")).collect();
    assert_eq!(names.len(), tools().len());           // no duplicates
    assert_eq!(names, ALL_CMDS.iter().map(|s| s.to_string()).collect()); // ALL_CMDS: test-only list of the 27 wire names, kept in step with the arms of `cmd_name`
}

#[test]
fn schemas_are_objects_listing_required_keys_that_examples_have() {
    for t in tools() {
        let s = (t.schema)();
        assert_eq!(s["type"], "object", "{}", t.cmd);
        for k in s["required"].as_array().into_iter().flatten() {
            assert!((t.example_args)().get(k.as_str().unwrap()).is_some(), "{} example lacks {k}", t.cmd);
        }
    }
}

#[test]
fn help_mentions_every_command_and_example() {
    let h = help_text();
    for t in tools() { assert!(h.contains(t.example), "{}", t.cmd); }
}
```

`ALL_CMDS` = status, list, platforms, holes, part, catalog, features, beams, bundles, snapshot, place, stack, place_bundle, constrain, move, set_params, rename, note, remove, undo, redo, book, book_list, book_add, book_remove, book_import, reload. (10 reads, 11 writes, 6 app commands.)

- [ ] **Step 2: Run, expect compile failure**

Run: `cargo test -p lab-app --offline --lib live::tools`
Expected: FAIL (module/functions not defined)

- [ ] **Step 3: Implement** `tools.rs`, `Request::cmd_name`, `pipe::pick`. Copy each command's usage, summary and example from today's `LIVE_HELP`. Schemas follow the arg structs in `protocol.rs`: `place` uses `oneOf` for its three placement forms; `hole` is `{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2}]}`; `to`/`parent` is `{"oneOf":[{"const":"room"},{"type":"object"}]}`.

- [ ] **Step 4: Switch the CLI to the table.** `LIVE_HELP` const goes; `Live::Help` prints `tools::help_text()`. `parse_live` ends with `tools::request_line(cmd, args, doc.as_deref())`; `run_live` uses `pipe::pick(pid)?`. Existing CLI tests stay unchanged and must pass.

- [ ] **Step 5: Run tests**

Run: `cargo test -p lab-app --offline --lib live:: && cargo test -p lab-app --offline --bin labsetup-cli`
Expected: PASS. Then compare `labsetup-cli live --help` with the old text: same commands, same examples.

- [ ] **Step 6: Commit** — `git add crates/lab-app/src/live crates/lab-app/src/bin/labsetup-cli.rs && git commit -m "Live control: one command table for help, requests and tools"`

---

### Task 2: MCP server (`labsetup-cli mcp`)

**Files:**
- Create: `crates/lab-app/src/live/mcp.rs` (protocol logic, no pipe; compiled on all platforms)
- Modify: `crates/lab-app/src/live/mod.rs` (`pub mod mcp;`)
- Modify: `crates/lab-app/src/bin/labsetup-cli.rs` (`mcp` command, `USAGE` line)

**Interfaces:**
- Consumes: `tools::{tools, find, request_line}`, `pipe::{pick, call}`.
- Produces:
  - `pub type SendFn<'a> = dyn FnMut(Option<u32>, &str) -> Result<String, String> + 'a;` — given pid and request line, returns the app's reply line or the error text.
  - `pub fn handle(msg: &Value, send: &mut SendFn) -> Option<Value>` — one JSON-RPC message in, response out (`None` for notifications).
  - `pub fn serve(input: impl BufRead, output: impl Write, send: &mut SendFn) -> io::Result<()>` — line loop until EOF; a line that isn't JSON gets JSON-RPC error `-32700`.
  - `pub const INSTRUCTIONS: &str`
  - CLI: `labsetup-cli mcp` runs `serve(stdin.lock(), stdout.lock(), &mut |pid, line| pick(pid).and_then(|n| call(&n, line, 2s).map_err(|e| format!("live: {e}"))))`. Nothing else may be printed to stdout.

Behavior:
- `initialize` → `{"protocolVersion": <client's if it is "2025-06-18" or "2025-03-26" or "2024-11-05", else "2025-06-18">, "capabilities":{"tools":{}}, "serverInfo":{"name":"labsetup","version":CARGO_PKG_VERSION}, "instructions":INSTRUCTIONS}`.
- `tools/list` → one tool per table row: `name` = `labsetup_` + snake cmd, `description` = summary + `\nExample: ` + example, `inputSchema` = schema plus optional `pid` (integer) on every tool and `doc` (string) on `Group::Write` tools.
- `tools/call` → strip `pid`/`doc` from arguments, `request_line`, `send`. App reply with `"ok":true` → `content:[{type:"text", text:<reply line>}]`; `"ok":false` → same with `isError:true`; `send` error → text = the error, `isError:true`. Unknown tool → JSON-RPC error `-32602`.
- `labsetup_snapshot` success: read `result.path`, return `[{type:"image", mimeType:"image/png", data:<base64>}, {type:"text", text:<reply line>}]`. Base64 is a private ~15-line encoder in `mcp.rs` (no dependency).
- `ping` → `{}`; unknown method with id → `-32601`.
- `INSTRUCTIONS` (keep under ~1200 chars): LabSetup is an optical-table layout app; call `labsetup_status` first and pass its `doc` token on every write; a `doc_changed` error means the user switched projects, so re-read status; use `labsetup_holes` before placing on a hole and `labsetup_features` before `labsetup_constrain`; units mm and degrees, rotations `[z,y,x]`; each write is one undo step the user can undo; check work with `labsetup_snapshot`; prefer `labsetup_place_bundle` and `labsetup_stack` over part-by-part; if a call says LabSetup isn't running, ask the user to open it and turn on *Allow AI agents* in Settings.

- [ ] **Step 1: Write the failing tests** in `mcp.rs` using a fake `send`:

```rust
#[test] fn initialize_returns_instructions_and_tools_capability()  // result.instructions == INSTRUCTIONS, capabilities.tools present, protocolVersion echoed for "2025-03-26"
#[test] fn tools_list_has_one_tool_per_command_with_pid_and_doc() // len == tools().len(); labsetup_place has doc+pid props; labsetup_status has pid, no doc
#[test] fn call_sends_the_cli_request_line()        // fake send records (pid, line); call labsetup_move {node:5, position:[10,0,0], doc:"T", pid:7} → pid Some(7), line == request_line("move", {node,position}, Some("T"))
#[test] fn error_reply_sets_is_error()              // send returns {"ok":false,...} → isError true
#[test] fn app_not_running_is_a_tool_error_and_server_continues() // send Err(NOT_RUNNING) → isError true, text == "LabSetup isn't running, or 'Allow AI agents' is off in Settings."; then a second call still answered (serve over two input lines)
#[test] fn snapshot_returns_inline_png()            // write a 3-byte file to temp, send returns ok with result.path → content[0].type == "image", data == base64 of the bytes ("AQID" for [1,2,3])
#[test] fn bad_json_line_gets_parse_error_and_notifications_get_no_reply()
#[test] fn base64_matches_rfc4648_vectors()         // "", "f"→"Zg==", "fo"→"Zm8=", "foo"→"Zm9v", "foobar"→"Zm9vYmFy"
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p lab-app --offline --lib live::mcp` → FAIL (not defined).
- [ ] **Step 3: Implement** `mcp.rs` and the `mcp` CLI command.
- [ ] **Step 4: Run tests** — same command → PASS.
- [ ] **Step 5: Smoke test by hand** with the app running and *Allow AI agents* on:

Run: `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"labsetup_status","arguments":{}}}' | target/release/labsetup-cli.exe mcp`
Expected: two response lines; the second has the app's status (doc token). With the app closed: second line has `isError:true` and the not-running message.

- [ ] **Step 6: Commit** — `git commit -m "Live control: MCP server (labsetup-cli mcp)"`

---

### Task 3: Agent registration (`lab_app::agents` + `labsetup-cli agents`)

**Files:**
- Create: `crates/lab-app/src/agents.rs`
- Modify: `crates/lab-app/src/lib.rs` (`pub mod agents;`)
- Modify: `crates/lab-app/src/bin/labsetup-cli.rs` (`agents install|uninstall|status`, `USAGE` line)

**Interfaces:**
- Produces:
  - `pub enum Format { McpServers /* {"mcpServers":{name:{command,args}}} */, VsCode /* {"servers":{name:{type:"stdio",command,args}}} */, Opencode /* {"mcp":{name:{type:"local",command:[exe,"mcp"],enabled:true}}} */, CodexToml /* [mcp_servers.labsetup] command = "…" args = ["mcp"] */ }`
  - `pub struct Agent { pub name: &'static str, pub config: PathBuf, pub format: Format }`
  - `pub fn known_agents(home: &Path, appdata: &Path) -> Vec<Agent>` — the spec's table. Detected = the config file exists, or its parent folder exists.
  - `pub enum Outcome { Added, Updated { was: String }, AlreadySet, Removed, NotSet, NotFound, Failed(String) }` with `Display` giving the words used in output: added / updated (was …) / already set / removed / not set / not found / failed: …
  - `pub fn merge(text: &str, format: Format, exe: &Path) -> Result<(String, Outcome), String>` — pure; empty text = new file
  - `pub fn remove(text: &str, format: Format) -> Result<(String, Outcome), String>` — pure
  - `pub fn current(text: &str, format: Format) -> Option<String>` — registered command path, if any
  - `pub fn install(exe: &Path, agents: &[Agent]) -> Vec<(&'static str, Outcome)>`, `pub fn uninstall(agents: &[Agent]) -> Vec<(&'static str, Outcome)>`, `pub fn status(exe: &Path, agents: &[Agent]) -> Vec<(&'static str, String)>` ("not found" / "not set" / "set" / "stale: <path>")
  - `pub fn snippet(exe: &Path) -> String` — the `mcpServers` JSON block for agents not in the table
  - `pub fn home_dirs() -> (PathBuf, PathBuf)` from `USERPROFILE` and `APPDATA`

Rules: back up the existing file to `<file>.labsetup-bak` before the first write that changes it; write with `lab_file::atomic_write`; `AlreadySet` and `NotFound` write nothing. Codex TOML is edited as text: find a line equal to `[mcp_servers.labsetup]` and replace through the line before the next line starting with `[` (or EOF); else append with a blank line before. TOML strings use single-quoted literal strings so Windows backslashes need no escaping.

- [ ] **Step 1: Verify the agent table.** For each agent in the spec's table, check its current docs for the user-level MCP config path and entry shape on Windows (Antigravity's path is unknown at writing). Fix `known_agents` to match; drop an agent that has no user-level file config, and note what was dropped in the commit message.

- [ ] **Step 2: Write the failing tests** in `agents.rs` (pure functions, temp dirs for install):

```rust
#[test] fn merge_into_empty_creates_entry()                 // McpServers: "" → mcpServers.labsetup == {command: exe, args:["mcp"]}, Added
#[test] fn merge_keeps_other_servers_and_settings()         // {"theme":"dark","mcpServers":{"other":{"command":"x"}}} → other unchanged, theme unchanged, labsetup added
#[test] fn merge_same_path_is_already_set_new_path_updates() // AlreadySet; then a different exe → Updated { was: old }
#[test] fn jsonc_or_broken_file_is_refused()                // "{ // c\n }" → Err; install leaves bytes identical, Outcome::Failed
#[test] fn vscode_and_opencode_shapes()                     // exact entry shapes from Format docs
#[test] fn codex_toml_append_replace_remove_keep_comments() // "# mine\n[model]\nx = 1\n" → appended table, "# mine" kept; second merge with new path replaces only that table; remove drops it and leaves the rest byte-identical
#[test] fn install_backs_up_then_uninstall_removes_only_ours() // temp home: file with other server → .labsetup-bak equals original; uninstall → labsetup gone, other kept, Removed
#[test] fn missing_agent_is_not_found_and_nothing_created()  // no folder → NotFound, no file written
#[test] fn status_reports_stale_path()                      // entry with other exe → "stale: <that path>"
```

- [ ] **Step 3: Run, expect failure** — `cargo test -p lab-app --offline --lib agents` → FAIL.
- [ ] **Step 4: Implement** `agents.rs` and the CLI subcommands. `install` prints `<agent>: <outcome>` per agent, then the snippet under "Other MCP-capable agents: add this to their MCP config:". Exit 0 unless any `Failed` (then 1).
- [ ] **Step 5: Run tests** — `cargo test -p lab-app --offline --lib agents && cargo test -p lab-app --offline --bin labsetup-cli` → PASS.
- [ ] **Step 6: Commit** — `git commit -m "Agent connect: register the MCP server in agent CLIs"`

---

### Task 4: Connect AI agents button in Settings

**Files:**
- Modify: `crates/lab-ui/ui/settings.slint` (under the *Allow AI agents* label)
- Modify: `crates/lab-app/src/settings_ui.rs` (`on_settings_connect_agents`)

**Interfaces:**
- Consumes: `lab_app::agents::{install, known_agents, home_dirs}`.
- Produces: Slint callback `connect-agents()` and property `agents-result: string`, exported as `settings-connect-agents` / `settings-agents-result` the same way the other settings properties are.

Behavior: button text "Connect AI agents…". Clicking runs `install(current_exe().with_file_name("labsetup-cli.exe"), &known_agents(..))` and puts one line per detected agent (skip `NotFound`) into `agents-result`, shown as a wrapped `Label` below the button. If `labsetup-cli.exe` isn't next to `labsetup.exe`: "labsetup-cli.exe wasn't found next to LabSetup; reinstall LabSetup." If no agent was found: "No coding-agent CLI found on this PC. Run 'labsetup-cli agents install' to see a config snippet for others." Add a last line when `allow_agents` is off: "Agents can connect once 'Allow AI agents' is On." The button works with the switch either way. *(Plan's call: inline result text instead of a separate dialog; the Settings window has no sub-dialog to copy.)*

- [ ] **Step 1: Implement** the Slint additions and the handler.
- [ ] **Step 2: Build** — `cargo build -p lab-app --offline --release` → builds.
- [ ] **Step 3: Check in the running app** (see `docs/agents/GUIDE.md` "Checking the running app"): open Settings, click the button, confirm the result lines match `labsetup-cli agents status`, and that Claude Code then lists `labsetup` (`claude mcp list`).
- [ ] **Step 4: Commit** — `git commit -m "Settings: Connect AI agents button"`

---

### Task 5: PATH in the installer, docs

**Files:**
- Modify: `installer/labsetup.wxs` (in the `MainExe` component: `<Environment Id="PathEntry" Name="PATH" Value="[INSTALLFOLDER]" Part="last" Action="set" System="yes" Permanent="no" />`)
- Modify: `README.md` (CLI section: `mcp` and `agents` commands; how to connect an agent)
- Modify: `CHANGELOG.md` (unreleased: "Connect AI agents" entry in the existing user-facing style)

- [ ] **Step 1: Edit** the three files.
- [ ] **Step 2: Build the MSI** — `powershell -File installer/build.ps1` (needs `wix.exe`; if absent, say so and skip). Expected: MSI builds without WiX errors.
- [ ] **Step 3: If an MSI was built,** install it, open a new terminal, run `labsetup-cli --version` from any folder → prints the version. Uninstall → the PATH entry is gone.
- [ ] **Step 4: End-to-end:** with the app open and the switch on, start a fresh Claude Code session in an unrelated folder and ask it to "list the parts in LabSetup". Expected: it calls `labsetup_status` / `labsetup_list` without being told where anything is.
- [ ] **Step 5: Commit** — `git commit -m "Installer: put LabSetup on PATH; docs for agent connect"`, then `graphify update .`
