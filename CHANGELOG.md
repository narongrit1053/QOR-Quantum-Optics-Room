# Changelog

## v0.0.7

- **Report a bug.** A bug button at the bottom right of the 3D view opens a new GitHub issue with the app version filled in.
- **Updates from inside the app.** LabSetup checks GitHub for a newer release once a day and shows an *Update* button at the top right. Click it to read what's new and install it. Turn the daily check off in the command palette (*Automatic Update Checks*), or check now with *Check for Updates*.
- **What's new.** This list opens once after each update, and any time from the command palette (*What's New*).

## v0.0.6

- **Faster Catalog window.** Catalog parts live in a local database, so the window opens without a pause, even with 20,000+ parts. The JSON files in the catalogs folder are imported in the background at startup.
- **Update catalog.** Refreshes the Thorlabs part list from thorlabs.com in the background, with thumbnails: Optomechanical Components, Fiber and Optical Elements, one request every 30 s. A full update takes one to two hours. Cancel keeps the progress for up to 7 days.
- If a section comes back empty or much smaller than last time, the catalog update is refused and the old parts are kept.
- A JSON file in the catalogs folder still overrides updated parts; the window tells you when it does.
- *Download & import list* is paused while a catalog update runs.

## v0.0.5

- **Relationships tree.** Select a part and the inspector lists only the parts linked to it by constraints or joints.
- **Product page links** for Thorlabs parts in your library.
- **Drag ghost** for parts imported from STEP.
- **Click again to deselect.**
- Fixes: a reversed slide range no longer crashes placement; Save no longer hangs when the 3D view stops answering.

## v0.0.4

- Bundles: save a set of parts to the library and place it anywhere.
- Selection lock; laser on/off toggle; a starter laser part.

## v0.0.3

- `labsetup-cli`, a command-line tool for project files.
- Constraints re-solve when you nudge or rotate a part from the keyboard.
