# Catalog database and in-app Thorlabs update

Status: draft for approval, 2026-11-21.

## Why

The user's Thorlabs catalog is a 15 MB JSON file (about 20,700 items, built with
`scripts/catalog/fetch-thorlabs.py`). Two problems with it:

1. **It loads slowly.** The first time the Catalog window opens, `lab_catalog::book::books()` reads
   and parses every `*.json` in `%APPDATA%\LabSetup\catalogs\` and builds a lower-cased search string per item,
   all on the UI thread, so the window freezes. (Not measured yet; the fix doesn't depend on which step dominates.)
2. **It is hard to update.** Updating means running a Python script from a terminal for one to two hours,
   then copying the output into the catalogs folder by hand.

The user considered an online library and chose a **local SQLite database plus an Update button in the app**
instead (option B). Rejected alternatives are listed under Approaches.

## Goals

- The Catalog window opens without a visible freeze. Results appear as you type, with a 20k-item catalog:
  under 100 ms to open and under 50 ms per search on the user's machine, release build.
- **Update catalog** in the Catalog window refreshes the Thorlabs part list in the background, with progress,
  while the user keeps working. Python is no longer needed, and nothing has to be copied by hand.
- Hand-written JSON catalogs (other vendors, e.g. MOGLabs or Toptica parts) keep working unchanged:
  the JSON format in `docs/CATALOG-FORMAT.md` stays the import format.

## Non-goals

- An online or shared server. The data stays on the user's PC.
- Updaters for vendors other than Thorlabs.
- Editing the fetched sections or exclusions in the UI. They are constants for now (see Decisions).
- Prices, stock or any other live data.
- Automatic or scheduled updates. Updates run only when the user presses the button.

## Anti-goals (failure even if it "works")

- The UI freezes during startup, opening the window, searching or updating.
- A network-capable crate ends up in `labsetup.exe` (`scripts/check-offline.ps1` must still pass).
  Network access goes through `curl.exe`, as thumbnails and STEP downloads already do (`import_url::curl`).
- A cancelled or failed update leaves the catalog half old and half new, or empty.
- Thorlabs data is committed to the repo, including in test fixtures. Fixtures are synthetic.
- The update hits thorlabs.com more often than once per 30 s (robots.txt `Crawl-delay: 30`).

## Approaches considered

| | Approach | Trade-off | Outcome |
|---|---|---|---|
| A | Keep JSON, load it in the background at startup, add an Update button that rewrites the JSON | Least code; the whole catalog stays in memory and every start reparses 15 MB | Not chosen |
| **B** | **Local SQLite database with full-text search; JSON files are imported into it; Update writes straight into it** | **Instant open and search with no startup parse; adds `rusqlite` and some code** | **Chosen by the user** |
| C | Online library on a server | Needs internet for every search and a server to run; hosting Thorlabs data for others is redistribution, which their copyright terms forbid | Rejected |

## Design

### Storage

- One database file: `%LOCALAPPDATA%\LabSetup\catalog.sqlite`. It is a rebuildable cache: deleting it costs
  a re-import of the JSON files plus a fresh update. It is not user content, so it doesn't go in `%APPDATA%`.
- Dependency: `rusqlite` with the `bundled` feature, which compiles SQLite from C source. No network code;
  the offline check must still pass. It goes in `lab-catalog` only.
- Schema (the builder may refine the column details):
  - `items(vendor, part, name, description, category_json, cat1, cat2, url, step, cad_pdf, image, source)`
    with primary key `(vendor, part, source)`. JSON rows and updater rows for the same part can coexist; reads
    pick the winner (see Precedence). `cat2` is the Catalog window's category filter (the second category entry,
    as today) and is indexed.
  - `source` is `json:<file name>` for items from a catalogs-folder file, or `update:thorlabs` for items
    written by the updater.
  - An FTS5 table over `part, name, description` with the **trigram** tokenizer. Today's search is a
    case-insensitive substring match (`"1509"` finds `LA1509`); trigram keeps that. Queries under 3 characters
    fall back to `LIKE`.
  - `imports(file, size, mtime)`, so unchanged JSON files aren't re-imported.
  - A `meta` table holding a schema version. On a version mismatch, the file is deleted and rebuilt.

### Precedence when the same (vendor, part) appears in several places

- JSON files win over the updater: a hand-written entry is a deliberate override.
- Between JSON files, the first by file name wins, as `load_books` does today.

### Startup and JSON import

- At startup, a background thread opens or creates the database. It compares each `*.json` in the catalogs folder
  against `imports`: changed files are re-imported (their old rows deleted first), and removed files have their
  rows deleted. Each file is imported in one transaction.
- A malformed file is skipped with a console message, as today, and its previous rows are kept.
- While this runs, the Catalog window shows "Loading catalog…" instead of freezing. In the normal case,
  nothing has changed and startup does only a few stat calls.
- **Migration:** the user's existing 15 MB `thorlabs.json` is imported once, like any other file.
  After the first successful in-app update, its items are duplicated by `update:thorlabs` rows. The JSON copy
  wins (see Precedence), so the window shows a one-line hint: "`thorlabs.json` overrides the updated
  Thorlabs parts. Remove it from the catalogs folder to use the update." The file is never deleted automatically.

### Catalog window

- Search and the category filter become SQL queries: `LIMIT 500` rows plus a count, as today
  ("showing 500 of N").
- Categories come from `SELECT DISTINCT cat2`.
- A part lookup is a primary-key query.
- The current `&'static BookItem` borrowing (`book::books()` and `import::start_book_batch(Vec<&'static BookItem>)`)
  changes to owned `BookItem`s loaded by part. The JSON-to-`BookItem` parsing stays, for import.
- The download list, thumbnails, Open page and Add to library keep their behaviour.
- New controls:
  - **Update catalog** button.
  - While an update runs: a progress line in the window ("Updating Thorlabs: Fiber 2,400 / 9,343") and a
    **Cancel** button.
  - When idle: "Thorlabs parts updated <date>", or "never".

### Updater (Rust port of `scripts/catalog/fetch-thorlabs.py`)

- Same queries and parsing as the script:
  - `products(query: <section>, first: 200, after: <offset>)` against `https://www.thorlabs.com/graphql`.
  - Keeps hits whose breadcrumb contains the section.
  - Category runs from the section down.
  - Drops excluded categories.
  - Searches each second-level category when a section exceeds the 10,000-hit cap.
  - Stores the `step` and `cad_pdf` asset links.
- **Sections and exclusions:** constants with the same defaults as the script.
  - Sections: Optomechanical Components, Fiber, Optical Elements.
  - Excluded: Enclosure Systems, Instrumentation Shelves, Optomechanics Kits, Optomechanics Capabilities.
- **Transport:** `import_url::curl` with `--data-binary @-` and a JSON body on stdin (the helper already supports
  stdin). One request at a time, at least 30 s apart, on a background thread.
- **Resume:** raw response pages are cached in `%LOCALAPPDATA%\LabSetup\catalog-update\` while an update runs.
  - A cache older than 7 days is discarded, and the update starts over with the message "Starting over: the saved
    progress is more than a week old." Pages are fetched by offset, so very old pages would mix with shifted
    new ones.
  - The time of the last request is stored, so a restart can't break the 30 s gap.
  - A lock file stops two running copies of LabSetup from updating at once.
  - Cancel, a quit or a failed request stops the update and keeps the cache; the next **Update catalog**
    continues from it.
  - A successful finish deletes the cache, so the next update fetches fresh data.
- **Atomic swap:** parsed items are written to a staging table, and the update finishes in one transaction:
  delete the `update:thorlabs` rows, then insert the staged ones. Cancel or failure leaves the old rows untouched.
- **Failure handling:**
  - A request is retried twice, with 60 s between tries.
  - After that, the update stops with a message in the window ("Update stopped: <reason>. Press Update to
    continue.").
  - If a page returns data that no longer parses as expected (the site changed), the update stops with a
    message saying so; it never swaps in a broken result.
- **Big-drop guard (user's decision):** each successful update stores its item count per section. Before the swap,
  if any section came back empty, or with fewer than 70% of its last count, the swap is refused and the old rows
  are kept. The message names the section and both counts: "Update stopped: Fiber returned 412 parts, last time
  5,696. The site may have changed; the old parts are kept." The first update has no previous counts, so only the
  "empty" check applies. The refused result is not kept; pressing Update again fetches fresh data.
- **Thumbnails (user's decision):** the search query also asks for `imgSrc` and `images { url name }`, in the
  same request, so it costs no extra requests. Each item's `image` is picked as the old Multica script did:
  - an image file whose name contains "thumb" or "small";
  - else `imgSrc`;
  - else the first `.jpg`, `.png` or `.webp`.
  The Catalog window already shows `image` thumbnails, downloading them through `curl.exe` into
  `%LOCALAPPDATA%\LabSetup\thumbs\` and caching them. That behaviour stays as is. The images are Thorlabs'
  copyright: they are cached for the user's own viewing only and never written into any exported or shared file.
- **Wording:** the window shows that a full update takes one to two hours.

### Python script

`scripts/catalog/fetch-thorlabs.py` stays as a command-line alternative. Its output is a JSON file, which the
import path still reads. `catalog/README.md` gets a short note that the app now has **Update catalog**, and that a
hand-copied `thorlabs.json` overrides it (see Migration).

## Testing

- `lab-catalog` unit tests:
  - JSON import, re-import on change and removal, using the existing synthetic `testdata/catalog-example.json`.
  - Precedence (JSON over update).
  - Substring search, including queries under 3 characters.
  - Category filter.
  - The atomic swap: cancelled staging leaves the old rows.
  - The big-drop guard: an empty section, a drop below 70%, and a first run with no previous counts.
  - Thumbnail choice from `images` and `imgSrc`.
- Updater parsing is tested against a small **synthetic** search-response fixture that has the real response's
  shape (breadcrumb JSON string, assets with groups) and made-up parts. It must not contain Thorlabs data.
- No test touches the network.
- Performance check, by hand in a release build:
  - Import the user's 20k-item file.
  - Open the Catalog window and time it.
  - Type a query and time it.

## Decisions made without the user (check these)

- The database lives in `%LOCALAPPDATA%` as a rebuildable cache, not in `%APPDATA%`.
- Hand-written JSON overrides updated rows, and `thorlabs.json` is never deleted automatically; a hint suggests removing it.
- Trigram full-text search, to keep today's substring matching.
- Sections and exclusions are constants, not settings.
- Update is manual only and resumable; Cancel keeps progress for up to 7 days.
- The 70% threshold for the big-drop guard (the guard itself is the user's decision).
- The Python script stays.

## First planning step

Measure where today's freeze comes from: the JSON parse and index build, or Slint laying out 500 rows. If Slint
layout is a large share, the database alone won't meet the 100 ms open target, and the plan must also cover the
row model (e.g. fewer initial rows or lazy thumbnails).

Measured 2026-11-21 (release build, 20,665-item `thorlabs.json`, Catalog opened from the start page, first open of the session):

| Step | Time |
|---|---|
| `book::books()` (read and parse the JSON) | 116 ms |
| First `search_book("", None)` (build the search index) | 46 ms |
| `refresh` (500 `CatalogRow`s took 0.5 ms; whole `refresh` 8 ms) | 8 ms |
| Next frame after `set_catalog_open(true)` (zero-delay timer) | 9.5 ms |

`open()` took 184 ms in total. JSON loading and index building (162 ms) are the freeze; `refresh` plus the next frame is
about 18 ms, under the 50 ms threshold, so Slint row drawing is not the problem and the database alone should meet the
100 ms open target. Task 7 (row model changes) is not needed. The zero-delay timer fires when the event loop is next
idle, so the next-frame number does not include GPU presentation; thumbnails were not cached yet (empty images).

## Left to the builder

- Exact SQL, indexes, the FTS setup, and how rows map to `BookItem`.
- Where the updater code lives (`lab-catalog` for parsing and storage, `lab-app` for the curl transport and UI is
  the expected split, since `curl` lives in `lab-app`).
- Thread and channel wiring for progress, following the existing thumbnail and import workers.
- Slint layout details for the new controls, in the Catalog window's existing style.
