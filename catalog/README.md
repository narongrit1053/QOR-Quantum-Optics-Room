# Catalogs

LabSetup ships no vendor catalog. You make your own: a JSON file listing the parts you use, in the format in [docs/CATALOG-FORMAT.md](../docs/CATALOG-FORMAT.md). This folder is a scratch place for those files. `*.json` and the fetch cache `.cache/` are git-ignored, so vendor data never gets committed.

To use a catalog, copy it to `%APPDATA%\LabSetup\catalogs\` (the **Open catalogs folder** button in the Catalog window opens it) and restart LabSetup.

## Thorlabs: fetch script

[scripts/catalog/fetch-thorlabs.py](../scripts/catalog/fetch-thorlabs.py) builds `catalog/thorlabs.json` from the thorlabs.com store search. Each request returns 200 items with part number, title, category path and STEP / CAD PDF links, so a few thousand items take minutes, not days.

```powershell
python scripts/catalog/fetch-thorlabs.py
```

By default it fetches **Optomechanical Components**, **Fiber** and **Optical Elements**, and leaves out Enclosure Systems, Instrumentation Shelves, Optomechanics Kits and Optomechanics Capabilities. Pick your own sections (any breadcrumb name from the site) and exclusions:

```powershell
python scripts/catalog/fetch-thorlabs.py --section "Fiber" --section "Optical Elements" --exclude "Optical Fiber & Fiber Patch Cables"
```

How it works:

- It searches the store for the section name and keeps hits whose breadcrumb contains that section. The search stops at 10,000 hits, so for a bigger section it also searches each second-level category it found.
- `category` runs from the section down, e.g. `["Fiber", "Optical Fiber & Fiber Patch Cables", "Polarizing Optical Fiber"]`. The second entry is what the Catalog window filters on.
- Raw pages are cached in `catalog/.cache/`. If a run is interrupted, run it again: cached pages are not fetched again. Delete the folder to get fresh data.
- Expect about 30 s per 200 items: roughly 25 min for Fiber (about 9,300 search hits), longer for Optical Elements.

## Thorlabs: what is allowed

Checked 2026-11-20 on thorlabs.com:

| Source | What it says | What it means for you |
|---|---|---|
| `robots.txt` | `Crawl-delay: 30`; `Disallow: /search`; `Disallow: /*.step$`, `/*.zip$`, `/*.dxf$` | Automated tools must wait 30 s between requests (the script does), must not use the site search pages, and must not fetch STEP, ZIP or DXF files. The script reads only `/graphql`, at one request per 30 s, and never downloads a STEP file. |
| [Legal → Copyright](https://www.thorlabs.com/legal) | Site texts, images, photos, videos and graphics are copyrighted; reproduction, editing or distribution without permission is prohibited. | The catalog holds part numbers, short titles, categories and links, for your own use. Don't publish or share the file, and don't add images or long product texts. Ask Thorlabs first if you want more. |

The script stores `step` and `cad_pdf` links but downloads nothing. Download each STEP file yourself (LabSetup's download list, or product page → CAD → **Step**), then import it with **Import STEP…** or **Import Folder into Library…**.

## Before you use the file

- LabSetup skips a file that is not valid JSON in this format and prints the reason to the console.
- The store search changes without notice. If the script stops finding items or fails, check one product page by hand before trusting the output.
- Prefer a few parts you name by hand? Write the JSON yourself in the format in [docs/CATALOG-FORMAT.md](../docs/CATALOG-FORMAT.md); the URL of any part is `https://www.thorlabs.com/item/<part>`.
