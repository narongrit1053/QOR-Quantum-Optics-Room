# Catalog Database and In-App Thorlabs Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the in-memory JSON catalog with a local SQLite database that opens instantly, and add an
**Update catalog** button that refreshes the Thorlabs parts, with thumbnails, in the background.

**Architecture:**
- `lab-catalog` gains a `db` module (schema, JSON import, search, update staging and swap with a big-drop guard)
  and a `thorlabs` module (search request bodies, response parsing, a query planner). Both are pure and unit-tested.
- `lab-app` gains a `catalog_update` worker that drives the planner over `curl.exe`, one request per 30 s, with a
  resume cache.
- `catalog_book.rs` switches from `book::books()` to database queries on a UI-thread connection, opened after a
  background startup sync.

**Tech Stack:** Rust; `rusqlite` with `bundled` (SQLite with FTS5 and the trigram tokenizer); Slint 1.18.1;
`curl.exe` via `import_url::curl`.

**Spec:** `docs/superpowers/specs/2026-11-21-catalog-database-design.md`

## Global Constraints

- `scripts/check-offline.ps1` must pass. `rusqlite` is used with `features = ["bundled"]` only, and all network
  access goes through `crate::import_url::curl` (`curl.exe`).
- Never more than one request to thorlabs.com per 30 s, including across app restarts.
- The database is `%LOCALAPPDATA%\LabSetup\catalog.sqlite`; the update cache is
  `%LOCALAPPDATA%\LabSetup\catalog-update\`; thumbnails stay in `%LOCALAPPDATA%\LabSetup\thumbs\`.
- No Thorlabs data in the repo. Test fixtures use made-up parts (`EX-…`) in the real response shape.
- Sections: `Optomechanical Components`, `Fiber`, `Optical Elements`. Excluded: `Enclosure Systems`,
  `Instrumentation Shelves`, `Optomechanics Kits`, `Optomechanics Capabilities`.
- Precedence: JSON rows beat `update:thorlabs` rows; between JSON files, the first file name wins.
- Big-drop guard: refuse the swap if any section has 0 items, or fewer than 70% of its last stored count.
- The UI thread never runs the JSON import, a network request, or anything that waits 30 s.
- User-facing text, exactly:
  - Loading: `Loading catalog…`
  - Progress: `Updating Thorlabs: {section} {done} / {total}`, with thousands separators, e.g. `Fiber 2,400 / 9,343`.
  - Idle, after an update: `Thorlabs parts updated {YYYY-MM-DD}`. Idle, never updated: `Thorlabs parts never updated`.
  - Duration note: `A full update takes one to two hours.`
  - Stop, generic: `Update stopped: {reason}. Press Update to continue.`
  - Stop, site changed: `Update stopped: Thorlabs search results are no longer in the expected format. The old parts are kept.`
  - Stop, big drop: `Update stopped: {section} returned {n} parts, last time {m}. The site may have changed; the old parts are kept.`
  - Expired resume cache: `Starting over: the saved progress is more than a week old.`
  - Override hint: `{file} overrides {n} updated Thorlabs parts. Remove it from the catalogs folder to use the update.`

## Review Focus

1. **Search text with FTS syntax characters** (`Ø1"`, `TR50/M`, `-`, `*`, `(`): must match as plain substrings,
   never cause an FTS syntax error. Test: `search_treats_query_as_literal` in Task 2.
2. **One- and two-character queries, and `%` or `_` in a query**: these fall back to `LIKE` and must match
   literally. Test: `short_queries_use_like_literally` in Task 2.
3. **A part in both a JSON file and the update**: it is listed and counted once, with the JSON version shown.
   Test: `json_beats_update_and_counts_once` in Task 3.
4. **The app quits or crashes mid-update**: the next run resumes from the cache, and a lock left behind by a dead
   run must not block updating forever. Test: `stale_lock_is_taken_over` in Task 5.
5. **The Catalog window is opened before the startup sync finishes**: it shows `Loading catalog…`, then fills
   itself without the user reopening it. Check: manual step in Task 6.

---

### Task 1: Measure where today's freeze comes from

**Files:**
- Modify (temporarily, reverted at the end of the task): `crates/lab-app/src/catalog_book.rs` (`open`, `refresh`)
- Record results in: `docs/superpowers/specs/2026-11-21-catalog-database-design.md`, under "First planning step"

- [ ] **Step 1:** In `open()`, time `book::books()`, the first `search_book("", None)` (index build), building
  the 500 `CatalogRow`s in `refresh`, and the time until the next frame. Use `std::time::Instant` and `eprintln!`
  lines prefixed `catalog-timing`. For the next frame, use `slint::Timer::single_shot(Duration::ZERO, …)` scheduled
  right after `set_catalog_open(true)`.
- [ ] **Step 2:** Build with `cargo build --release -p lab-app`. Run `target\release\labsetup.exe` with the
  user's 20k-item `%APPDATA%\LabSetup\catalogs\thorlabs.json` in place. Open the Catalog window
  (`scripts/dev/lab.ps1`: `Start-Lab`, then the File menu → Catalog…) and copy the `catalog-timing` lines.
- [ ] **Step 3:** Write the four numbers into the spec's "First planning step". If `refresh` plus the next frame
  is more than 50 ms, Task 7 applies; otherwise mark Task 7 as not needed.
- [ ] **Step 4:** Revert the instrumentation (`git checkout crates/lab-app/src/catalog_book.rs`) and commit the
  spec change: `git commit -m "Catalog: measure Catalog window open time"`.

### Task 2: `CatalogDb` with schema, JSON import and search

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]`: add `rusqlite = { version = "<latest>", features = ["bundled"] }`,
  pinned to the version cargo resolves), `crates/lab-catalog/Cargo.toml`, `crates/lab-catalog/src/lib.rs`
  (`pub mod db;`, `pub fn catalog_db_path() -> Option<PathBuf>` = `%LOCALAPPDATA%\LabSetup\catalog.sqlite`)
- Create: `crates/lab-catalog/src/db.rs`
- Test: inline `#[cfg(test)] mod tests` in `db.rs`, using `testdata/catalog-example.json` and temp dirs

**Interfaces:**
- Consumes: `book::parse_book(&str) -> serde_json::Result<Vec<BookItem>>`, `BookItem`.
- Produces:
  - `pub struct CatalogDb`
  - `CatalogDb::open(path: &Path) -> Result<CatalogDb, rusqlite::Error>`: creates the schema. If `meta.schema`
    differs from `SCHEMA_VERSION: i64 = 1`, deletes the file and recreates it. Sets WAL mode.
  - `CatalogDb::open_in_memory() -> Result<CatalogDb, rusqlite::Error>`
  - `sync_json_dir(&mut self, dir: &Path) -> SyncReport`. `pub struct SyncReport { pub imported: Vec<String>, pub removed: Vec<String>, pub skipped: Vec<(String, String)> }`, with file names and error text.
  - `search(&self, query: &str, cat2: Option<&str>, limit: usize) -> (Vec<BookItem>, usize)`: the rows and the
    total match count.
  - `categories(&self) -> Vec<String>`: distinct `cat2` values, sorted, excluding empty.
  - `find(&self, part: &str) -> Option<BookItem>`
  - `is_empty(&self) -> bool`
  - Internal: `recompute_winners(&self)`, which sets `items.winner` (the row shown per `(vendor, part)`) after
    any import or swap.

Schema, as the spec says. Notes:
- Primary key `(vendor, part, source)`.
- `items.winner INTEGER` and an index on `(winner, cat2)`.
- `items_fts` is an FTS5 table, `tokenize='trigram'`, over `part, name, description`, with `content='items'` and
  `content_rowid='rowid'`.
- `imports(file PRIMARY KEY, size, mtime)` and `meta(key PRIMARY KEY, value)`.
- Search ordering: `ORDER BY part COLLATE NOCASE`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn imports_example_and_searches() {
    // sync a temp dir holding catalog-example.json
    assert_eq!(report.imported, ["catalog-example.json"]);
    assert_eq!(db.search("ex-km", None, 500).0[0].part, "EX-KM1");
    assert_eq!(db.search("", Some("Posts"), 500).1, 1);
    assert_eq!(db.categories(), ["Lenses", "Mirror Mounts", "Posts"]);
    assert_eq!(db.find("EX-L100").unwrap().image.as_deref(), Some("https://example.com/img/EX-L100.png"));
}
#[test] fn unchanged_file_is_not_reimported() { /* second sync: imported is empty */ }
#[test] fn changed_and_removed_files() { /* rewrite with one item → 1 row; delete file → removed == [name], is_empty() */ }
#[test] fn malformed_file_keeps_previous_rows() { /* overwrite with "{" → skipped has it, search still finds EX-KM1 */ }
#[test] fn first_file_name_wins() { /* a.json and b.json share EX-KM1 with different names → search shows a.json's */ }
#[test] fn search_treats_query_as_literal() {
    // items named `Mount for Ø1" Optics` and part `TR50/M`
    for q in ["Ø1\"", "tr50/m", "(", "*", "-"] { db.search(q, None, 500); } // no panic, no Err
    assert_eq!(db.search("ø1\"", None, 500).1, 1);
    assert_eq!(db.search("TR50/M", None, 500).1, 1);
}
#[test] fn short_queries_use_like_literally() {
    assert_eq!(db.search("m", None, 500).1, /* items containing m */);
    assert_eq!(db.search("%", None, 500).1, 0);
    assert_eq!(db.search("_", None, 500).1, 0);
}
#[test] fn schema_version_mismatch_rebuilds() { /* set meta.schema = 0, reopen → empty, no error */ }
```

- [ ] **Step 2:** Run `cargo test -p lab-catalog db::`. Expected: compile failure (no `db` module).
- [ ] **Step 3:** Implement `db.rs`.
  - Queries of 3 or more characters: `items_fts MATCH ?`, with the query wrapped as one FTS string, `"` doubled
    inside.
  - Shorter queries: `LIKE ? ESCAPE '\'` over `lower(part||char(10)||name||char(10)||description)`, with `%`, `_`
    and `\` escaped.
  - Each file is imported in one transaction: delete its rows, insert, update `imports`, then `recompute_winners`.
- [ ] **Step 4:** Run `cargo test -p lab-catalog`. Expected: PASS. Then run
  `powershell -ExecutionPolicy Bypass -File scripts/check-offline.ps1`. Expected: `Offline check passed`.
- [ ] **Step 5:** Commit: `git commit -m "lab-catalog: SQLite catalog database with JSON import and search"`

### Task 3: Update staging, swap, big-drop guard and override hint

**Files:**
- Modify: `crates/lab-catalog/src/db.rs`

**Interfaces:**
- Produces:
  - `pub const UPDATE_SOURCE: &str = "update:thorlabs";`
  - `stage_begin(&mut self)`: clears the `staging` table.
  - `stage_insert(&mut self, section: &str, items: &[BookItem])`: insert-or-ignore by `(vendor, part)`.
  - `finish_update(&mut self, sections: &[&str], now_unix: i64) -> Result<usize, DropRefused>`
    - `pub struct DropRefused { pub section: String, pub got: usize, pub last: usize }`
    - Checks each section's staged count against `meta["count:{section}"]`.
    - On pass, in one transaction: deletes the `UPDATE_SOURCE` rows, inserts the staged ones, stores the new
      counts and `meta["updated"] = now_unix`, clears staging, then runs `recompute_winners`. Returns the item count.
    - On refusal: clears staging and changes nothing else.
  - `last_updated(&self) -> Option<i64>`
  - `meta_get(&self, key: &str) -> Option<String>` and `meta_set(&self, key: &str, value: &str)`, used by Task 5
    for `last_request`.
  - `overrides(&self) -> Vec<(String, usize)>`: JSON files whose rows beat `UPDATE_SOURCE` rows, with counts.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn first_update_only_refuses_empty() { /* no counts stored; section with 0 → Err; 1 item → Ok(1) */ }
#[test] fn drop_below_70_percent_is_refused() {
    // last count 100: 69 → Err(DropRefused{section:"Fiber", got:69, last:100}); 70 → Ok
}
#[test] fn refused_update_keeps_old_rows() { /* old update rows still searchable after Err */ }
#[test] fn swap_replaces_update_rows() { /* EX-OLD gone, EX-NEW present, last_updated == Some(now) */ }
#[test] fn json_beats_update_and_counts_once() {
    // JSON file and update both have EX-KM1 → search("ex-km1").1 == 1, name is the JSON one,
    // overrides() == [("catalog-example.json".into(), 1)]
}
```

- [ ] **Step 2:** Run `cargo test -p lab-catalog db::`. Expected: FAIL (missing methods).
- [ ] **Step 3:** Implement as specified above.
- [ ] **Step 4:** Run `cargo test -p lab-catalog`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "lab-catalog: staged catalog update with big-drop guard"`

### Task 4: Thorlabs search parsing and query planner

**Files:**
- Create: `crates/lab-catalog/src/thorlabs.rs`,
  `crates/lab-catalog/src/testdata/thorlabs-search-page.json` (synthetic, `EX-…` parts)
- Modify: `crates/lab-catalog/src/lib.rs` (`pub mod thorlabs;`)

**Interfaces:**
- Produces:
  - Constants: `SECTIONS: [&str; 3]`, `EXCLUDE: [&str; 4]`, `PAGE_SIZE: usize = 200`, `SEARCH_CAP: usize = 10_000`,
    `GRAPHQL_URL: &str = "https://www.thorlabs.com/graphql"`.
  - `request_body(query: &str, offset: usize) -> String`: the JSON POST body.
    - The GraphQL text is the script's `QUERY` (`scripts/catalog/fetch-thorlabs.py`), plus `imgSrc images { url name }`
      in `items`.
    - Variables `q`, `n = PAGE_SIZE`, `a = offset` as a string.
  - `pub struct SearchPage { pub total: usize, pub items: Vec<BookItem>, pub second_levels: BTreeSet<String> }`
  - `parse_page(json: &str, section: &str, exclude: &[&str]) -> Result<SearchPage, FormatChanged>`
    - `pub struct FormatChanged(pub String)`.
    - Returns `Err` when `data.products.totalCount` or `items` is missing.
    - Item mapping as `to_item` in the script: `vendor = "Thorlabs"`, and the URL keeps `ItemUrl`.
    - `image` is chosen in this order:
      1. an `images` entry whose name matches `(?i)thumb|small` and ends `.jpg`, `.jpeg`, `.png` or `.webp`;
      2. else `imgSrc`, if it has one of those extensions (query string allowed);
      3. else the first `images` entry with one of those extensions.
  - `pub struct Request { pub section: String, pub query: String, pub offset: usize }`
  - `pub struct Planner`
    - `Planner::new(sections: &[&str]) -> Planner`
    - `next(&self) -> Option<Request>`
    - `accept(&mut self, page: &SearchPage)`. After the last page of a section's own query, when
      `total > SEARCH_CAP`, its `second_levels` (sorted) are queued as further queries for that section.
    - `progress(&self) -> (String /*section*/, usize /*done*/, usize /*total*/)`
  - `cache_name(query: &str, offset: usize) -> String`: lowercase slug of `[^a-z0-9]+` → `-`, then
    `-{offset:06}.json`, as in the script.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn parses_fixture() {
    let p = parse_page(FIXTURE, "Fiber", &EXCLUDE).unwrap();
    assert_eq!(p.total, 3);
    assert_eq!(p.items.len(), 1); // one in an excluded category, one outside the section
    assert_eq!(p.items[0].category, ["Fiber", "Fiber Components", "EX Couplers"]);
    assert_eq!(p.items[0].step.as_deref(), Some("https://example.com/ex-fc1.step"));
    assert_eq!(p.items[0].image.as_deref(), Some("https://example.com/ex-fc1_thumb.jpg"));
    assert!(p.second_levels.contains("Fiber Components"));
}
#[test] fn image_falls_back_to_imgsrc_then_first_image() { /* variants built inline with serde_json::json! */ }
#[test] fn missing_products_is_format_changed() { assert!(parse_page(r#"{"data":{}}"#, "Fiber", &[]).is_err()); }
#[test] fn planner_pages_and_expands_over_cap() {
    // section total 10_400 → offsets 0..9_800 step 200, then second-level queries in sorted order
}
#[test] fn cache_name_matches_script() { assert_eq!(cache_name("Optical Elements", 400), "optical-elements-000400.json"); }
```

- [ ] **Step 2:** Run `cargo test -p lab-catalog thorlabs::`. Expected: FAIL.
- [ ] **Step 3:** Implement `thorlabs.rs`. Breadcrumb handling: `Breadcrumb` is a JSON string of `[name, path]`
  pairs, leaf first. Drop the last two entries ("All Products" and the item), reverse, and cut from `section`.
- [ ] **Step 4:** Run `cargo test -p lab-catalog`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "lab-catalog: Thorlabs search parsing and query planner"`

### Task 5: Update worker in `lab-app`

**Files:**
- Create: `crates/lab-app/src/catalog_update.rs`
- Modify: `crates/lab-app/src/main.rs` or `lib.rs` (`mod catalog_update;`, wherever `mod catalog_book;` is
  declared)

**Interfaces:**
- Consumes:
  - `thorlabs::{Planner, Request, parse_page, request_body, cache_name, SECTIONS, EXCLUDE, GRAPHQL_URL}`
  - `CatalogDb::{open, stage_begin, stage_insert, finish_update, meta_get, meta_set}`
  - `crate::import_url::curl(args, Some(body))`
- Produces:
  - `pub enum UpdateEvent { Progress { section: String, done: usize, total: usize }, Notice(String), Finished(usize), Stopped(String) }`.
    `Notice` carries the "Starting over…" text. `Stopped` carries the full user-facing text from Global Constraints.
  - `pub fn start(on_event: impl Fn(UpdateEvent) + Send + 'static) -> bool`: false if this process or another
    process (lock) is already updating.
  - `pub fn cancel()`
  - `pub fn running() -> bool`
  - Pure helpers, tested:
    - `fn wait_before_next(last_request_unix: Option<i64>, now_unix: i64) -> u64`, the seconds to sleep, ≥ 30 s
      gap.
    - `fn cache_expired(started_unix: i64, now_unix: i64) -> bool`, true after 7 days.
    - `fn lock_is_stale(lock_mtime_unix: i64, now_unix: i64) -> bool`, true when not touched for 5 minutes. The
      worker touches the lock before each request.

Worker loop:
1. Take the lock (`catalog-update\.lock`, `create_new`, or take over a stale one).
2. If `catalog-update\started` is older than 7 days, clear the cache and send a `Notice`.
3. `stage_begin`, then for each `planner.next()`:
   - use the cached page if present;
   - otherwise sleep `wait_before_next`, POST via `curl(&["-X","POST","-H","Content-Type: application/json","--data-binary","@-",GRAPHQL_URL], Some(&body))`
     (3 tries, 60 s apart), store `meta last_request`, and write the cache file;
   - then `parse_page` → `stage_insert` → `accept` → `Progress`.
   - Check `cancel` between requests and during sleeps, at least once a second.
4. At the end, `finish_update`. On `Ok`, delete the cache directory, then send `Finished`. On `DropRefused`, delete
   the cache too (the spec says the refused result is not kept, so the next Update fetches fresh data), then send
   the big-drop text.
5. Always remove the lock.

The `FormatChanged` and curl failures map to the `Stopped` texts. The worker opens its own `CatalogDb` connection.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn waits_at_least_30_seconds() {
    assert_eq!(wait_before_next(None, 1000), 0);
    assert_eq!(wait_before_next(Some(990), 1000), 20);
    assert_eq!(wait_before_next(Some(900), 1000), 0);
}
#[test] fn cache_expires_after_seven_days() {
    assert!(!cache_expired(0, 7 * 86_400));
    assert!(cache_expired(0, 7 * 86_400 + 1));
}
#[test] fn stale_lock_is_taken_over() {
    assert!(!lock_is_stale(1000, 1000 + 299));
    assert!(lock_is_stale(1000, 1000 + 301));
}
```

- [ ] **Step 2:** Run `cargo test -p lab-app catalog_update`. Expected: FAIL.
- [ ] **Step 3:** Implement `catalog_update.rs` as described above.
- [ ] **Step 4:** Run `cargo test -p lab-app catalog_update`. Expected: PASS.
- [ ] **Step 5:** Commit: `git commit -m "Catalog: background Thorlabs update worker"`

### Task 6: Catalog window on the database, with the Update controls

**Files:**
- Modify:
  - `crates/lab-app/src/catalog_book.rs`: replace `book::books()`, `search_book`, `filter`, `find` and
    `categories()` with `CatalogDb` calls. Keep the thumbnails, the list and `connect` verbs.
  - `crates/lab-app/src/import.rs`: change `Job`'s `Option<&'static BookItem>` to `Option<BookItem>`, and
    `start_book_batch(ui, items: Vec<BookItem>)`.
  - `crates/lab-app/src/controller.rs`: start the background sync at startup.
  - `crates/lab-ui/ui/app.slint` and `crates/lab-ui/ui/catalog_book.slint`: new properties and controls.
  - `crates/lab-catalog/src/book.rs`: remove `books()` and `load_books()`. `parse_book` stays.
- Test: update `catalog_book_filter` to run against `CatalogDb::open_in_memory()` plus a synthetic JSON file, or
  delete it if Task 2's tests cover the same cases. Note which in the commit.

**Interfaces:**
- Consumes:
  - Tasks 2–3 `CatalogDb` API and `lab_catalog::catalog_db_path()`
  - Task 5 `catalog_update::{start, cancel, running, UpdateEvent}`
- Produces:
  - Slint properties: `in property <bool> catalog-loading; in property <bool> catalog-updating;
    in property <string> catalog-update-status; in property <string> catalog-hint;`
  - New `catalog-cmd` verbs: `"update"` and `"cancel-update"`.
  - `pub fn startup_sync(ui: &AppWindow)`, called from `controller.rs` next to `catalog_book::connect`:
    1. Spawns a thread that runs `CatalogDb::open` and `sync_json_dir(catalogs_dir)` and prints the `SyncReport`
       skips to the console.
    2. Then, via `upgrade_in_event_loop`, opens the UI-thread connection, stores it in a `thread_local!`, clears
       `catalog-loading`, and calls `refresh` if the window is open.

UI behaviour:
- **Loading:** while `catalog-loading`, the window shows `Loading catalog…` in place of the results.
- **Header row:** an **Update catalog** `PillButton`, next to the List and Grid buttons. While updating, it becomes
  **Cancel**.
- **Status line** under the header row, in `Theme.text-tertiary`. It shows one of:
  - the progress or stop text;
  - when idle: `Thorlabs parts updated …` or `Thorlabs parts never updated`, then ` · A full update takes one to
    two hours.`
- **Hint:** `catalog-hint` holds the override text for the first entry of `overrides()`; empty when there are none.
- **Events:** each `UpdateEvent` is applied through `upgrade_in_event_loop`. `Finished` refreshes the rows,
  categories and hint.
- **`catalog-empty`:** true only when the database has no rows and no update is running.

- [ ] **Step 1:** Change `import.rs` to owned `BookItem` and run `cargo test -p lab-app import::`. Expected: PASS.
  `book_parts_take_the_book_category` is unchanged.
- [ ] **Step 2:** Switch `catalog_book.rs` to `CatalogDb` and add `startup_sync`. Run `cargo test -p lab-app`.
  Expected: PASS.
- [ ] **Step 3:** Add the Slint properties and controls. Run `cargo build -p lab-app`. Expected: builds.
- [ ] **Step 4: Manual check** (release build, `scripts/dev/lab.ps1`). Expected for each:
  - Open the Catalog window within 1 s of start: `Loading catalog…`, then rows appear by themselves.
  - Reopen it: it appears without a visible freeze.
  - Type `km1`: results update.
  - Press **Update catalog**: the progress line appears after the first request.
  - Press **Cancel**: `Update stopped`. Press **Update catalog** again: it resumes from cached pages, which you
    can count in `catalog-update\`.
  - With `thorlabs.json` present: the hint names it.
- [ ] **Step 5:** Run `scripts/check-offline.ps1`. Expected: `Offline check passed`. Then commit:
  `git commit -m "Catalog: database-backed Catalog window with Update catalog"`

### Task 7 (only if Task 1 found `refresh` plus the next frame over 50 ms): faster first paint

**Files:**
- Modify: `crates/lab-app/src/catalog_book.rs` (`MAX_ROWS`, `refresh`)

- [ ] **Step 1:** Change `MAX_ROWS` from 500 to 200, and the count text stays `showing 200 of N`.
  `fetch_thumbs` already only queues missing files; no change there.
- [ ] **Step 2:** Re-run Task 1's timing on the database build. Expected: open plus the first frame is under
  100 ms. If it isn't, stop and report the numbers. Don't guess further optimizations.
- [ ] **Step 3:** Commit: `git commit -m "Catalog: fewer initial rows for a faster open"`

### Task 8: Docs

**Files:**
- Modify:
  - `catalog/README.md`: add a short "In the app" section at the top. It says **Update catalog** in the Catalog
    window does the same as the script, in the background. It also says a `thorlabs.json` in the catalogs folder
    overrides the update, so remove it after the first update. The script stays documented as the
    command-line alternative.
  - `docs/CATALOG-FORMAT.md`: in "Where the file goes", say that the files are imported into a local database at
    startup, and that changes are picked up on the next start.
  - `README.md`: one line about **Update catalog**.

- [ ] **Step 1:** Edit the three files.
- [ ] **Step 2:** Commit: `git commit -m "Docs: catalog database and Update catalog"`
