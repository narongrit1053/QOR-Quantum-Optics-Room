# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Quantum-optics labs: physicists, graduate students and lab managers who plan optical-table experiments before they buy parts or bolt anything down. They come to this site to judge whether LabSetup fits that job and to download it.

## Product Purpose

LabSetup is a native, offline Windows app for designing quantum-optics lab setups: rooms, optical tables, breadboards and custom platforms, mounting hardware, optics and lasers, shown in 3D and as a dimensioned top view. Success is a lab that can lay out an experiment, check beam heights and clearances, and export a parts list before spending money or bench time.

## Positioning

A dedicated desktop planner for optical-table layouts that runs entirely offline, keeps projects as plain files on disk, traces the beam through the placed optics, and imports the vendor CAD (STEP) the lab already uses. It ships without vendor catalogs; users bring their own catalog JSON.

## Operating Context

- Windows 10 or 11, 64-bit, DirectX 12 GPU. Portable zip: unzip and run `labsetup.exe`; no installer, no network.
- Projects are `.labsetup` files; sample projects live in `samples/` on this site.
- Typical work: lay out a table, stack posts and mounts to a beam height, constrain parts to each other (mate, flush, concentric, angle), import Thorlabs-style STEP parts, export a CSV bill of materials and a PNG view.

## Capabilities and Constraints

- 3D, Top and Split views; beam tracing; part library with search and filters; drag-and-drop placement onto table holes; constraints with a per-part relationships tree; custom platform outlines; room editor with doors and windows; STEP / URL / folder import; CSV and PNG export; `labsetup-cli` for project files.
- Free download, closed source. Do not claim open source.
- Early preview: pre-release v0.0.x builds. Expect rough edges and changes between versions; say so plainly.
- Releases are published on GitHub (`narongrit1053/QOR-Quantum-Optics-Room`) as a Windows x64 portable zip.

## Brand Commitments

- Name: LabSetup.
- Thorlabs is a trademark of Thorlabs, Inc.; LabSetup is not affiliated with Thorlabs. Keep this notice wherever Thorlabs is named.

## Evidence on Hand

- Screenshots in `img/` (from the v0.0.1 and v0.0.2 builds): hero, top view, split view, catalog, library search, stack builder, custom platform, room editor, constraints, vendor parts, PNG export.
- Sample projects and an example parts list in `samples/`.
- No testimonials, user counts, publications or institutional endorsements exist. Do not invent any.

## Product Principles

1. Offline and local: nothing leaves the user's machine.
2. Show the real app: real screenshots and real sample files, not mockups.
3. Honest about maturity: an early preview, stated up front.
4. Lab vocabulary: beam height, hole grid, post stack, mount; written for people who work at an optical table.
