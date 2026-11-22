# Live catalog download and reload — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a live-control agent search the downloadable catalog, manage the download list, run Download & import, and reload catalog files and the part library without restarting; recompute the beam whenever the library loads.

**Architecture:** A third request kind, `Request::App(AppRequest)`, joins `Read` and `Write` in the live protocol (lib crate). `agent.rs` (bin crate) answers it on the UI thread at once, unpaced and without undo, by calling new public functions in `catalog_book.rs`, `import.rs` and `library.rs`. The window and live control share `catalog_book`'s list state.

**Tech Stack:** Rust, Slint, serde_json, rusqlite (`lab_catalog::db`).

**Spec:** `docs/superpowers/specs/2026-11-22-live-catalog-design.md`

## Global Constraints

- Build env (Git Bash): `export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH`; always `--offline`; never change dependencies or features.
- Wire names: `book`, `book_list`, `book_add`, `book_remove`, `book_import`, `reload` (CLI: `-` for `_`).
- `book*`/`reload` make no undo entry, need no doc token, are not paced.
- Mutating app requests (`book_add`, `book_remove`, `book_import`, `reload`) are refused `busy` while `agent::is_busy` is true or `catalog_update::running()`; `book` and `book_list` are always answered.
- `book_add`: unknown part → `not_found`; part without STEP → `refused`; already listed → no change, not an error.
- Unknown args are `bad_request` (add each command to `ARGS` in `protocol.rs`).

## Review Focus

1. `book_add` with a mix of good and bad parts: nothing is added and the error names the first bad part (all-or-nothing, like other agent steps).
2. `book_import` with an empty list, or every listed part already in the library: replies `{started:false, count:0}` and clears done parts, never opens the import sheet.
3. `book_import` while an import is already running: `busy`, never a second batch.
4. `reload` when a catalog JSON is invalid: still reloads the rest and reports the file in `skipped`, no error reply.
5. `book_list` during an import: `running:true` with `done`/`total`; after it ends, `running:false` and `last_summary` set.

---

### Task 1: Protocol — `AppRequest`

**Files:**
- Modify: `crates/lab-app/src/live/protocol.rs` (enums ~L19-52, `ARGS`, `parse_line` ~L229)
- Test: same file's `#[cfg(test)]` module

**Interfaces:**
- Produces: `pub enum AppRequest { Book { search: String, limit: Option<usize> }, BookList, BookAdd { parts: Vec<String> }, BookRemove { parts: Vec<String> }, BookImport, Reload }` (serde tag `cmd`, content `args`, snake_case); `Request::App(AppRequest)`; `pub fn AppRequest::mutates(&self) -> bool` (false for `Book`, `BookList`).

- [ ] **Step 1: Failing tests** `app_requests_parse`: `{"v":1,"cmd":"book_add","args":{"parts":["PBS122"]}}` → `Request::App(AppRequest::BookAdd{parts:vec!["PBS122".into()]})`; `{"v":1,"cmd":"reload"}` → `Request::App(AppRequest::Reload)`; `{"v":1,"cmd":"book","args":{"search":"pbs","colour":1}}` → `Err` with code `bad_request`; `AppRequest::BookList.mutates() == false`, `AppRequest::BookImport.mutates() == true`.
- [ ] **Step 2:** `cargo test --offline -p lab-app --lib app_requests_parse` → fails to compile.
- [ ] **Step 3:** Add the enum and variant; in `parse_line` try `ReadRequest`, then `WriteRequest`, then `AppRequest` (keep the "report the one that knew it" error choice across all three); add `ARGS` rows `("book", &["search","limit"])`, `("book_add", &["parts"])`, `("book_remove", &["parts"])`, `("book_list", &[])`, `("book_import", &[])`, `("reload", &[])`.
- [ ] **Step 4:** Test passes; `cargo test --offline -p lab-app --lib live::` all pass.
- [ ] **Step 5:** Commit `live: app requests for the catalog and reload`.

### Task 2: Beam recomputed when the library loads

**Files:**
- Modify: `crates/lab-app/src/library.rs` (`reload_catalog` ~L524), `crates/lab-app/src/main.rs` (after `state.catalog = …` ~L114)

- [ ] **Step 1:** In `reload_catalog`, call `crate::beams::retrace(app)` after the catalog is replaced; in `main.rs`, call `crate::beams::retrace(&mut state)` after `state.catalog` is set (before `refresh_all`).
- [ ] **Step 2:** `cargo build --offline -p lab-app` succeeds; `cargo test --offline -p lab-app --bin labsetup beams` passes.
- [ ] **Step 3:** Manual check (Task 5): open a project with a laser; `live beams` is non-empty without any edit.
- [ ] **Step 4:** Commit `Beams: retrace when the part library loads`.

### Task 3: Catalog list, import and reload API

**Files:**
- Modify: `crates/lab-app/src/catalog_book.rs`, `crates/lab-app/src/import.rs` (`BATCH`, `end_batch` ~L568)
- Test: `catalog_book.rs` `#[cfg(test)]`

**Interfaces (all `pub`, UI thread, bin crate):**
- `catalog_book::edit_list(list: &mut Vec<String>, add: bool, parts: &[String], has_step: impl Fn(&str) -> Option<bool>) -> Result<(), (ErrorCode, String)>` — pure; `has_step` returns `None` for unknown parts. All-or-nothing.
- `catalog_book::add_parts(ui: &AppWindow, app: &AppState, parts: &[String], add: bool) -> Result<Vec<String>, (ErrorCode, String)>` — `edit_list` on `STATE.list` with `find`, `save_list`, `refresh` if the window is open; returns the new list.
- `catalog_book::search(app: &AppState, query: &str, limit: usize) -> Vec<serde_json::Value>` — `{vendor, part, name, has_step, in_library, listed}` per row (reuse `in_library`, `with_db(|db| db.search(query, None, limit))`).
- `catalog_book::list() -> Vec<String>`.
- `catalog_book::start_import(ui: &AppWindow, app: &AppState) -> Result<usize, String>` — the body of the `"import"` branch in `connect` moved here (that branch now calls it); `Ok(0)` when nothing is left to fetch; `Err` while `catalog_update::running()`.
- `catalog_book::resync() -> Vec<(String, String)>` — `sync_json_dir` on the UI connection (`DB.with(|d| d.borrow_mut())`), returns skipped `(file, error)`; then `show_db`-style refresh if the window is open.
- `import::progress() -> Option<(usize, usize, usize)>` — `(done, total, failed)` of the running batch; `import::last_summary() -> Option<String>` — set in `end_batch`.

- [ ] **Step 1: Failing tests** `edit_list_rules`: starting `["A"]`, `has_step` = `A→Some(true), B→Some(true), C→Some(false)`, others `None`: add `["B"]` → `["A","B"]`; add `["B","Z"]` → `Err(NotFound, msg containing "Z")` and list unchanged; add `["C"]` → `Err(Refused, …)`; add `["A"]` → `Ok`, list unchanged; remove `["A","Q"]` → `["B"]` (removing an unlisted part is fine).
- [ ] **Step 2:** `cargo test --offline -p lab-app --bin labsetup edit_list_rules` fails.
- [ ] **Step 3:** Implement the interfaces above; `ErrorCode` from `lab_app::live::protocol`.
- [ ] **Step 4:** Test passes; window still works: build, open Catalog…, Add to list / Download & import unchanged (manual in Task 5).
- [ ] **Step 5:** Commit `Catalog: shared list/import/reload API for live control`.

### Task 4: Agent routing and CLI

**Files:**
- Modify: `crates/lab-app/src/agent.rs` (`admit` ~L183, `on_line` ~L217), `crates/lab-app/src/bin/labsetup-cli.rs` (`LIVE_HELP`, `STRING_KEYS`)
- Test: `agent.rs` tests, `labsetup-cli.rs` tests

**Interfaces:**
- Consumes: Task 1 `AppRequest`, Task 3 functions, `library::reload_catalog`, `library::sync_ui`.
- Produces: replies — `book`: `[{vendor,part,name,has_step,in_library,listed}]`; `book_list`: `{list, running, done, total, failed, last_summary}`; `book_add`/`book_remove`: `{list}`; `book_import`: `{started, count}`; `reload`: `{skipped:[{file,error}], warnings:[…], parts:N}`.

- [ ] **Step 1: Failing tests** in `agent.rs`: `app_read_ignores_busy` (`admit` admits `book_list` with busy=true and a stale doc token); `app_write_while_busy_is_busy` (`book_add` with busy=true → `Busy`); `app_write_ignores_doc_token` (`reload` with doc `"old"`, busy=false → Ok). In the CLI: `live book-add --parts '["PBS122","KM100"]'` → args `{"parts":["PBS122","KM100"]}`, cmd `book_add`, and the wire line parses.
- [ ] **Step 2:** Run both test sets; they fail.
- [ ] **Step 3:** `admit`: `Request::App(r)` → Ok if `!r.mutates()`, else `Busy` when busy (no doc check). `on_line`: `Request::App(r)` answered immediately via `answer`; `busy` also when `catalog_update::running()` for mutating ones and when `import::progress().is_some()` for `book_import`/`reload`. `reload` = `catalog_book::resync()` then `library::reload_catalog` + `library::sync_ui` + `controller::refresh`. Add the six commands to `LIVE_HELP` (an "App" section: no undo, no --doc needed).
- [ ] **Step 4:** All `lab-app` tests pass: `cargo test --offline -p lab-app`.
- [ ] **Step 5:** Commit `live: book and reload commands`.

### Task 5: End-to-end check in a running app

- [ ] **Step 1:** `cargo build --offline -p lab-app` (debug). Copy `target/debug/labsetup.exe` and `labsetup-cli.exe` to the scratchpad; run with `APPDATA`/`LOCALAPPDATA` pointed at scratch copies of `%APPDATA%\LabSetup` (settings with `allow_agents: true`, `catalogs\`) and `%LOCALAPPDATA%\LabSetup\catalog.sqlite`, opening a copy of `PDH NIMT optics.labsetup`.
- [ ] **Step 2:** `live beams` is non-empty right after start (Task 2).
- [ ] **Step 3:** Add a part line to a scratch catalog JSON, `live reload`, `live book --search <it>` finds it.
- [ ] **Step 4:** `live book --search "beam dump"`, `live book-add --parts '["<one with STEP>"]'`, `live book-import`, poll `live book-list` until `running:false`; `live catalog --search <part>` now lists it; `live place` it.
- [ ] **Step 5:** Review Focus 1–4 by hand: mixed add, empty import, import while running, `reload` with a broken JSON file in the scratch catalogs folder (reply lists it in `skipped`).
- [ ] **Step 6:** Commit nothing new; report results. Release build for the user is a separate step after review.
