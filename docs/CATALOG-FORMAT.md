# Catalog format

LabSetup ships **no vendor catalog data**. The **Catalog…** window (File menu, start page, library menu) lists only the catalogs you add yourself: JSON files you write by hand or generate with an AI assistant.

## Where the file goes

Put each catalog in `%APPDATA%\LabSetup\catalogs\` as a `.json` file (any name). The **Open catalogs folder** button in the Catalog window creates and opens that folder. LabSetup reads every `*.json` there when the Catalog window is first opened in a session; restart LabSetup after adding a file. A file that is not valid JSON in this format is skipped (the reason is printed to the console), and the other files still load. The same `(vendor, part)` in several files is listed once.

## Schema

```json
{
  "vendor": "string, required",
  "items": [ { ...item... } ]
}
```

| Item field | Required | Meaning |
|---|---|---|
| `part` | yes | Vendor part number, e.g. `EX-KM1` |
| `name` | yes | Short product name |
| `url` | yes | Product page (opened by **Open page**) |
| `category` | yes | Path from general to specific, e.g. `["Optomechanics", "Mirror Mounts"]`. The second entry is the category filter. May be `[]`. |
| `description` | no | Longer text; searched along with part and name |
| `step` | no | Direct link to the STEP file. Without it the part cannot be added to the download list. |
| `cad_pdf` | no | Link to a CAD drawing PDF |
| `image` | no | Link to a `.jpg`, `.png` or `.webp` thumbnail |

## Example

```json
{
  "vendor": "Example Optics",
  "items": [
    {
      "part": "EX-KM1",
      "name": "Kinematic Mirror Mount, 25 mm",
      "description": "Two-adjuster kinematic mount for 25 mm optics",
      "category": ["Optomechanics", "Mirror Mounts"],
      "url": "https://example.com/parts/EX-KM1",
      "step": "https://example.com/cad/EX-KM1.step"
    },
    {
      "part": "EX-P50",
      "name": "Optical Post, 50 mm",
      "category": ["Optomechanics", "Posts"],
      "url": "https://example.com/parts/EX-P50"
    },
    {
      "part": "EX-L100",
      "name": "Plano-Convex Lens, f = 100 mm",
      "description": "N-BK7, 25 mm diameter, uncoated",
      "category": ["Optical Elements", "Lenses"],
      "url": "https://example.com/parts/EX-L100",
      "cad_pdf": "https://example.com/cad/EX-L100.pdf",
      "image": "https://example.com/img/EX-L100.png"
    }
  ]
}
```

(The same file is the test fixture `crates/lab-catalog/src/testdata/catalog-example.json`.)

## AI prompt

Copy this, fill in the parts, and save the answer as a `.json` file in the catalogs folder:

```text
Create a LabSetup catalog JSON for these parts: <vendor>, <part numbers or a description of the parts>.
Use exactly this format and output only the JSON:
{"vendor": "<vendor>", "items": [{"part": "...", "name": "...", "description": "...",
"category": ["<general>", "<specific>"], "url": "<product page>",
"step": "<direct STEP download link, or omit>", "cad_pdf": "<or omit>", "image": "<or omit>"}]}
Required per item: part, name, url, category. Only use links you are sure exist; omit a field rather than guess.
```

Check the links before you rely on them: assistants can invent URLs.

## Vendor notes

You are responsible for respecting each vendor's terms of use and copyright. LabSetup ships no vendor data; a catalog you make is for your own use.

- **Thorlabs:** `url` is `https://www.thorlabs.com/item/<part>`. Copy the STEP and CAD PDF links from the product page.
