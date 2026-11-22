# Catalog format

LabSetup ships **no vendor catalog data**. The **Catalog…** window (File menu, start page, library menu) lists the catalogs you add yourself (JSON files you write by hand or generate with an AI assistant) and, after **Update catalog**, the Thorlabs parts it fetched.

## Where the file goes

Put each catalog in `%APPDATA%\LabSetup\catalogs\` as a `.json` file (any name). The **Open catalogs folder** button in the Catalog window creates and opens that folder. At startup LabSetup imports every `*.json` there into a local database (`%LOCALAPPDATA%\LabSetup\catalog.sqlite`) on a background thread. Only changed files are re-imported, and the parts of removed files are dropped. Changes are picked up on the next start, so restart LabSetup after adding or changing a file. A file that is not valid JSON in this format is skipped (the reason is printed to the console), and the other files still load. The same `(vendor, part)` in several files is listed once.

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

- **Thorlabs:** `url` is `https://www.thorlabs.com/item/<part>`. [scripts/catalog/fetch-thorlabs.py](../scripts/catalog/fetch-thorlabs.py) builds a whole catalog, with `step` and `cad_pdf` links, from the store search; site rules and usage are in [catalog/README.md](../catalog/README.md).

## Optical port kinds and node parameters (built-in TOML parts)

An `[[optical_ports]]` entry's `kind` is one of `mirror`, `lens`, `bs` (50/50 beamsplitter), `pbs` (polarizing beamsplitter), `hwp`, `qwp`, `isolator` (ideal Faraday isolator, passes along the port normal), `source`, `detector`, `dump`, `fiber`. The old `splitter` and `waveplate` still load (as `bs` and a pass-through).

The beam trace reads these numeric node parameters: `wp_angle_deg` (waveplate fast-axis angle) and `iso_angle_deg` (isolator input polarizer angle). Both angles are in degrees from lab horizontal relative to the port normal (perpendicular to the normal and room +Z; the port frame `x` if the normal is vertical), rotated about the port normal, positive counter-clockwise as seen by the receiver of a beam travelling along the normal (looking back toward the source).
