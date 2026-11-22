# Live control: download catalog parts and reload without restarting

## Why

Building the PDH NIMT setup through `labsetup-cli live` stalled every time a part had to come from the
downloadable catalog: only the user could press **Add to list** and **Download & import** in the Catalog
window, and every catalog JSON added to `%APPDATA%\LabSetup\catalogs\` or part file edited in the user
library needed a LabSetup restart (and a save first, because the agent can't save). The agent should be
able to finish such a job alone, with the user watching.

## Goals

- An agent can search the downloadable catalog (`catalog.sqlite`), put parts on the download list, run
  Download & import, and see when it is done and what failed.
- An agent can make LabSetup re-read the catalog JSON files and the part library on demand.
- The beam trace is recomputed whenever the part library is reloaded (today it stays empty after a
  project opens until some part is edited).

## Non-goals

- Watching the folders for changes automatically (considered; see Approaches).
- Running the full Thorlabs **Update catalog** from live control.
- Letting the agent save the project.

## Anti-goals

- An import that the user cannot see: it runs through the same import sheet and toast as the window's
  button.
- Agent edits landing mid-import: while the import sheet is open, document writes stay `busy`, as now.

## Approaches

1. **`reload` command (picked by the user).** The agent calls it after changing files. Small, explicit,
   no background work.
2. **Automatic folder watching.** Nothing to remember, but more code, always running, and it can reload a
   file half written.

## Design

New live commands (wire name = CLI name with `-` → `_`):

| cmd | args | does |
|---|---|---|
| `book` | `search`, `limit?` | read: downloadable-catalog rows `{vendor, part, name, has_step, in_library, listed}` |
| `book_list` | | read: the download list, and the import state `{running, done, total, failed, last_summary}` |
| `book_add` | `parts` (array of part numbers) | puts parts with a STEP link on the list (saved to `catalog-list.json`); unknown part → `not_found`, no STEP → `refused` |
| `book_remove` | `parts` | takes parts off the list |
| `book_import` | | starts Download & import for the list, like the window button; replies at once `{started, count}`; `busy` if an import or a catalog update is running |
| `reload` | | re-syncs the catalog JSON folder into the database, reloads the part library, recomputes the beam; replies with skipped files and library warnings |

- `book*` and `reload` change app state, not the document: no undo entry, no doc token needed, not paced.
  They are refused with `busy` while an import or catalog update runs (except the two reads).
- The window and live control share one list (`catalog_book` state), so the Catalog window shows what
  the agent added.
- `reload` runs the JSON sync on the existing database connection; it only re-imports changed files.
- Beam fix: `library::reload_catalog` also calls `beams::retrace`; the startup path gets the same retrace.
- `labsetup-cli live --help` documents the new commands.

## Left to the builder

- Exact reply field names beyond those above, and how the import progress counters are read from the
  running batch.
- Whether `reload` refreshes the Catalog window rows if it is open (it should if cheap).
- Tests: protocol parsing for each command, list add/remove rules, and a manual end-to-end run: search,
  add, import, `book_list` until done, place the part.
