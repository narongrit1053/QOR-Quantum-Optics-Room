# Changelog

## Unreleased

- **A LabSetup slash command.** Agents that show MCP prompts as slash commands get one for LabSetup (in Claude Code: `/mcp__labsetup__labsetup`, with what you want built). It starts the agent on your open project with the working rules.
- **More agent apps.** *Connect AI agents…* now also adds LabSetup to the Claude desktop app (not only Claude Code), Trae, opencode and Hermes Agent. Restart an agent after connecting so it loads LabSetup.

## v0.0.13

- **Any AI agent can find LabSetup.** Click *Connect AI agents…* in Settings, and coding agents on this PC (Claude Code, Codex, Gemini CLI, Antigravity, Cursor, Copilot CLI, Windsurf, Devin, VS Code) get LabSetup as a tool. Open the agent in any folder, ask it to build your setup, and it already knows how; no need to point it at `labsetup-cli` or explain the commands. Agents can also see a picture of the view.
- **Agents follow your lead.** Every agent gets the same rules when it connects: do only what you asked, use only catalog and library parts (importing a real part, never building a look-alike), ask before removing parts or downloading, and never undo your own edits.
- **Agents can place your saved bundles.** An agent can list your library bundles and drop one onto a hole in one step, instead of rebuilding the assembly part by part.
- **`labsetup-cli` works from any folder.** The installer adds LabSetup to `PATH`.

## v0.0.12

- **Group a single stack.** Ctrl+G now works when everything you selected is one stack (for example base → post holder → post → mount), and makes it a group folder. Before, it said "Select at least two parts to group".
- **Save as Bundle saves when you press Enter.** Choosing *Save as Bundle…* from the right-click menu left the name field without focus, so Enter did nothing and no bundle was saved.
- **Bundles show the whole set.** A bundle's library picture and the preview that follows the mouse while you drag it now show every part, not just the first. A dropped bundle selects all its parts.

## v0.0.11

- **Select several parts.** Ctrl+click parts in the view or in the left panel to add or remove them; Shift+click in the left panel selects a range. Delete, Lock, Hide, Isolate, Copy, Cut, Duplicate and the arrow keys act on all of them. The inspector shows the first one.
- **Groups.** Select parts and press Ctrl+G (or right-click → Group) to keep them together. A group shows as a folder in the left panel, and clicking any of its parts in the view selects the whole group; click again to pick just that part. Dragging or nudging moves the group as one rigid block, and one Ctrl+Z puts it back. Ctrl+Shift+G ungroups.
- **Save a whole assembly as a bundle.** *Save as Bundle…* now saves everything selected, or the whole group, including parts held by constraints, so a stack like base → post holder → post → mount → mirror is saved complete from any of its parts. A bundle saved from a group comes back as a group when you drop it in.

## v0.0.10

- **Version and updates in Settings.** Settings shows which LabSetup you have, with a *Check for Updates* button. When a newer version is out, the button turns into *Update*, which downloads and installs it.
- **Orbiting a selected part keeps it in view.** The camera now turns around the middle of the whole part, and *Frame Selection* fits all of it. Before, some imported parts swung out of view when you orbited them.

## v0.0.9

- **Thorlabs polarization optics from the catalog now work.** Polarizing beamsplitter cubes (such as PBS122) act as a PBS, and half- and quarter-wave plates (such as WPH10M and WPQ10M) turn the polarization, with a *Fast axis* angle in the inspector. Before, parts imported from the catalog split the beam 50/50 and left the polarization unchanged.

## v0.0.8

- **AI agents can build your setup.** Turn on *Allow AI agents to control LabSetup* in Settings, and an agent on this PC can place parts, stack posts and mounts at a beam height, and add constraints through `labsetup-cli live`. You watch each step appear, and Ctrl+Z undoes it.
- **Agents can fetch catalog parts.** An agent can search the catalog, put parts on the download list and run *Download & import*, the same list and progress you see in the Catalog window.
- **No restart after adding catalog files.** An agent can ask LabSetup to re-read the catalogs folder and the part library (`labsetup-cli live reload`).
- **Polarization.** Beams carry their polarization. Mirrors, polarizing and plain beam splitters (PBS, BS), half- and quarter-wave plates act on it, and the inspector shows it for each pass of the beam.
- **Faraday isolator.** A new library part that passes light one way and blocks the return beam.
- **Beam Profiler.** In the Top view, click a beam to drop a probe that shows the spot size, polarization and power at that point.
- **Path lengths through returning beams.** A Michelson arm now reads its true length.
- The library's right-click menu is clickable again.
- **Beams show right after opening a project.** The beam trace no longer stays empty until you edit a part.

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
