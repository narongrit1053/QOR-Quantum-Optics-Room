# Catalogs

LabSetup ships no vendor catalog. You make your own: a JSON file listing the parts you use, in the format in [docs/CATALOG-FORMAT.md](../docs/CATALOG-FORMAT.md). This folder is a scratch place for those files; `*.json` here is git-ignored so vendor data never gets committed.

To use a catalog, copy it to `%APPDATA%\LabSetup\catalogs\` (the **Open catalogs folder** button in the Catalog window opens it) and restart LabSetup.

## Thorlabs: what is allowed

Checked 2026-11-20 on thorlabs.com:

| Source | What it says | What it means for you |
|---|---|---|
| `robots.txt` | `Crawl-delay: 30`; `Disallow: /search`; `Disallow: /*.step$`, `/*.zip$`, `/*.dxf$` | Automated tools must wait 30 s between pages, must not use site search, and must not fetch STEP, ZIP or DXF files. Product pages (`/item/<part>`) are allowed. |
| [Legal → Copyright](https://www.thorlabs.com/legal) | Site texts, images, photos, videos and graphics are copyrighted; reproduction, editing or distribution without permission is prohibited. | Keep a catalog to part numbers, names, short descriptions and links, for your own use. Don't copy long product texts or images, and don't publish or share the file. Ask Thorlabs first if you want more. |

That is why there is **no fetch script** here: a crawler over the whole catalog would need days at one page per 30 s, and STEP downloads are off-limits to bots anyway (on the product page they are buttons, not links). Instead:

- Let an AI assistant with web access read **only the product pages for part numbers you name** (prompt below). A few dozen parts is fine.
- Download each **STEP file yourself**: product page → CAD → **Step**. Then import them in LabSetup with **Import STEP…**, or all at once with **Import Folder into Library…**.

## Prompt: parts you know

Replace the part list, paste the prompt into an assistant that can browse the web (Claude with web search, for example), and save its answer as `catalog/thorlabs.json`.

```text
Build a LabSetup catalog JSON for these Thorlabs parts:
KM100, KM100/M, TR50/M, PH50/M, BA1/M, LA1509, PF10-03-P01

Rules for using the Thorlabs website:
- Open only the product page of each part: https://www.thorlabs.com/item/<part number>
  (write "/" in a part number as-is, e.g. https://www.thorlabs.com/item/KM100/M).
- Do not use the site search, do not crawl other pages, and wait at least 30 seconds
  between page loads (thorlabs.com robots.txt asks for this).
- Do not download STEP, ZIP or DXF files.
- Copy only facts: part number, product name, a one-line description in your own words,
  and the category. Do not copy long marketing text or images.

Output only JSON, in exactly this format:
{
  "vendor": "Thorlabs",
  "items": [
    {
      "part": "<part number exactly as Thorlabs writes it>",
      "name": "<product name from the page title>",
      "description": "<one short line in your own words: size, thread, key spec>",
      "category": ["<general, e.g. Optomechanics>", "<specific, e.g. Mirror Mounts>"],
      "url": "https://www.thorlabs.com/item/<part number>"
    }
  ]
}

Use the page's breadcrumb for the category: its first useful level as the first entry,
the most specific level as the second. Required per item: part, name, url, category.
Leave out the optional fields "step", "cad_pdf" and "image": on thorlabs.com they are
download buttons, not links. If a part number has no product page, leave it out and
list it after the JSON under "Not found:".
```

## Prompt: a family of parts

When you don't know the part numbers yet, ask for the list first, then use the prompt above:

```text
List the Thorlabs part numbers for <family, e.g. "metric Ø1/2" optical posts, all lengths">.
Use only your own knowledge or at most 3 Thorlabs product pages (https://www.thorlabs.com/item/<part>),
with at least 30 seconds between page loads, and no site search.
Answer as one comma-separated line of part numbers, then one line per part number
you are unsure of, starting with "Unsure:".
```

## Before you use the file

- Assistants can invent part numbers and links. Open a few `url`s to check them, and drop any part in the "Not found" list.
- LabSetup skips a file that is not valid JSON in this format and prints the reason to the console.
