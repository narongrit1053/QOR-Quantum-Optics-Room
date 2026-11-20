---
name: LabSetup
description: Download page for a free, offline Windows app that plans quantum-optics lab setups in 3D and top view.
colors:
  ground: "#0c0d10"
  ground-2: "#121419"
  raise: "#181b21"
  line: "rgba(222, 228, 240, 0.09)"
  line-strong: "rgba(222, 228, 240, 0.16)"
  text: "#eceef2"
  text-2: "#a9aebb"
  text-3: "#8a90a0"
  accent: "#0b66d6"
  accent-hover: "#0a5cc2"
  link: "#5fa8ff"
  beam: "#ff4f9a"
typography:
  display:
    fontFamily: "Geist, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(2.6rem, 6.2vw, 4.6rem)"
    fontWeight: 600
    lineHeight: 1.02
    letterSpacing: "-0.04em"
  headline:
    fontFamily: "Geist, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "clamp(1.8rem, 3.4vw, 2.6rem)"
    fontWeight: 600
    lineHeight: 1.1
    letterSpacing: "-0.03em"
  title:
    fontFamily: "Geist, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "1.3rem"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.015em"
  body:
    fontFamily: "Geist, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "Geist, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    fontFeature: "tnum"
  code:
    fontFamily: "Cascadia Code, Consolas, monospace"
    fontSize: "0.88em"
    fontWeight: 500
rounded:
  code: "5px"
  thumb: "10px"
  button-small: "9px"
  button: "11px"
  window: "14px"
  pill: "999px"
spacing:
  gutter: "16px"
  bar-height: "60px"
  container: "1160px"
  section: "clamp(80px, 11vw, 128px)"
  row-gap: "clamp(64px, 9vw, 112px)"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "#ffffff"
    rounded: "{rounded.button}"
    padding: "13px 22px"
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
    textColor: "#ffffff"
  button-small:
    backgroundColor: "{colors.accent}"
    textColor: "#ffffff"
    rounded: "{rounded.button-small}"
    padding: "7px 14px"
  top-bar:
    backgroundColor: "rgba(12, 13, 16, 0.78)"
    textColor: "{colors.text-2}"
    height: "{spacing.bar-height}"
  window-frame:
    backgroundColor: "#000000"
    rounded: "{rounded.window}"
  status-pill:
    textColor: "{colors.text-2}"
    rounded: "{rounded.pill}"
    padding: "6px 14px"
  inline-code:
    backgroundColor: "{colors.raise}"
    textColor: "#d9dde6"
    rounded: "{rounded.code}"
    typography: "{typography.code}"
---

# Design System: LabSetup

## Overview

**Creative North Star: "The Lit Workbench"**

A near-black, cool-tinted graphite room where the real app window is the only bright object, lit from behind by a soft blue falloff. The page is the category-standard dark product page, built to Raycast's level of finish: centered hero copy, a large framed screenshot, alternating feature rows, a quiet file list, and a download button reachable at every scroll depth. Nothing about the arrangement is novel; the craft is in the hairlines, the shadows under the windows, the restrained single accent, and type set tight at display sizes.

Density is calm and editorial: generous section padding, measured line lengths (46 to 76ch), and one family doing every job. Imagery is always the real application, framed as a window; the page never draws a mockup of the product.

**Key Characteristics:**
- One cool graphite ground with two tonal steps; depth comes from soft shadows under screenshot windows, not from cards.
- One accent, the app's selection blue, deepened for white-text contrast on buttons.
- Geist only, self-hosted, weight 400 to 600, negative tracking at display sizes.
- Hairline dividers at 9% and 16% white separate sections and list rows.
- Real app rasters in window frames carry all the visual weight.

## Colors

A cool graphite neutral ramp with one blue accent and a single pink signal reserved for the laser beam.

### Primary
- **Selection Blue** (accent): the only filled action color. Download buttons, the bullet dots of the primary change list. Deepened from the app's own selection blue so white button text passes contrast.
- **Pressed Selection Blue** (accent-hover): button hover only.
- **Signal Link Blue** (link): inline links, file-list hover, and the focus ring. The lighter sibling of the accent, legible as text on the dark ground.

### Tertiary
- **Beam Pink** (beam): the laser path in the wordmark glyph. It stands for the beam the app traces; it is not a UI color.

### Neutral
- **Graphite Ground** (ground): page background, top bar tint.
- **Lifted Graphite** (ground-2): top of the vertical gradient on the What's-new band.
- **Raised Graphite** (raise): inline code chips.
- **Hairline** (line) and **Strong Hairline** (line-strong): section dividers, list rules, window and pill borders.
- **Paper White** (text): headings, strong text, file names.
- **Muted Steel** (text-2): body copy under headings, nav links, captions in figures.
- **Quiet Steel** (text-3): meta lines, captions, file descriptions, footer.

### Named Rules
**The One Blue Rule.** Selection Blue is the only filled color on the page. Every filled shape in it is a download action or a primary-list marker; nothing decorative is blue-filled.

**The Beam Is Not A Brand Color Rule.** Beam Pink appears only where it depicts the laser beam. Never use it for buttons, links, borders or highlights.

## Typography

**Display Font:** Geist (with Segoe UI Variable Text, Segoe UI, system-ui)
**Body Font:** Geist, same stack
**Label/Mono Font:** Cascadia Code (with Consolas, monospace) for inline code only

**Character:** One grotesk carries everything, from tight 600-weight display headlines to 17px reading text, so hierarchy comes from size, weight and tracking rather than from a second family.

### Hierarchy
- **Display** (600, clamp 2.6 to 4.6rem, 1.02): the hero headline only. Tracking -0.04em, balanced wrapping.
- **Headline** (600, clamp 1.8 to 2.6rem, 1.1): one per section.
- **Title** (600, 1.3rem, 1.25): feature-row headings. A quieter variant (500, 1.05rem, Muted Steel) titles secondary lists.
- **Body** (400, 17px, 1.6): reading text, held to 46 to 70ch. Ledes and row text in Muted Steel.
- **Label** (400, 14px): meta lines, captions, footer, pill text; tabular numerals on meta lines with sizes and versions.

### Named Rules
**The One Family Rule.** Geist is the only typeface for prose and headings; monospace appears only inside inline code.

**The No Kicker Rule.** Headings stand alone. No eyebrow labels, small-caps tags or section numbers above them.

## Layout

A single centered container of 1160px with a 16px side gutter at every width (`min(1160px, 100% - 32px)`). The hero screenshot breaks slightly wider (1240px). Sections stack vertically with clamp padding of 80 to 128px and a hairline between neighbors.

The hero is centered text over a full-width window. Feature rows are two-column grids at 5:8, alternating sides (8:5 flipped), with 64 to 112px between rows. Secondary features sit in a two-column figure grid under a hairline. The What's-new band pairs a change list with a 300px sticky inspector screenshot; samples pair a 5:7 heading column with a ruled file list.

At 860px and below every grid collapses to one column, the nav links hide (wordmark and Download remain), the sticky inspector screenshot becomes static, and the hero window tightens to 12px side margins with a 10px radius. Phones under 640px get a narrower hero crop and a shortened preview pill.

## Elevation & Depth

Flat surfaces, lifted pictures. The page itself has no cards and no raised panels; depth exists only under screenshot windows and buttons, as long, soft, black shadows that read as the window floating above the graphite. Blue radial light behind the hero and behind the final download section supplies atmosphere.

### Shadow Vocabulary
- **Window float** (`0 0 0 1px rgba(0,0,0,0.6), 0 30px 80px -20px rgba(0,0,0,0.8), 0 10px 24px -8px rgba(0,0,0,0.6)`): every framed screenshot.
- **Hero float** (`0 0 0 1px rgba(0,0,0,0.6), 0 40px 100px -30px rgba(0,0,0,0.9), 0 12px 30px -10px rgba(0,0,0,0.7)`): the hero window only.
- **Thumb float** (`0 18px 40px -16px rgba(0,0,0,0.7)`): secondary feature images.
- **Button** (`0 1px 0 rgba(255,255,255,0.2) inset, 0 6px 16px -6px rgba(0,0,0,0.7), 0 1px 3px rgba(0,0,0,0.5)`): primary buttons; deepens on hover with a 1px lift.

### Named Rules
**The Only The Window Floats Rule.** Shadows belong to screenshots and buttons. Text blocks, lists and sections stay flat on the ground and are separated by hairlines.

## Shapes

Softly rounded rectangles that echo a desktop window: 14px on framed screenshots (12px on the tall inspector crop, 10px on thumbnails and on the narrow hero), 11px on buttons (9px small), 5px on code chips, full pill on status notes. Borders are 1px hairlines in Strong Hairline. Icons are 18px inline SVG line drawings at 1.5 to 1.8 stroke with round caps.

## Components

### Buttons
- **Shape:** gently rounded (11px; 9px small).
- **Primary:** Selection Blue fill, white 600-weight 16px text, 13px by 22px padding, optional leading 18px download icon, inset top highlight plus soft drop shadow.
- **Hover / Focus:** Pressed Selection Blue, 1px lift, deeper shadow, 0.2s on the expo-out ease; returns to rest on active. Focus is the shared 2px Signal Link Blue ring offset 3px.
- **Small:** 7px by 14px, 14px text; used in the top bar.

### Top Bar
- Sticky, 60px tall, Graphite Ground at 78% with saturate-and-blur backdrop, hairline bottom border.
- Wordmark left (24px glyph with the beam drawn in Beam Pink), text nav links in Muted Steel at 15px turning Paper White on hover, small Download button right. Nav links hide below 860px.

### Window Frame (signature)
- A real app screenshot in a black frame, 14px radius, Strong Hairline border, Window float shadow, clipped overflow. The hero instance is wider, uses Hero float, and rises in on load (blur and 28px translate, 1.1s, disabled for reduced motion).

### Change List
- Unstyled list with 22px left inset and a 6px Selection Blue dot per item; bold lead phrase in Paper White, the rest in Muted Steel. The quiet variant uses a 5px Quiet Steel dot, tighter gap and 15.5px text.

### Feature Row
- Text column (Title plus Muted Steel paragraph at 46ch) beside a Window Frame, alternating sides, vertically centered.

### File List
- Rows ruled by hairlines top and bottom, 16px vertical padding. File link in Paper White 500 with a Quiet Steel file icon; both turn Signal Link Blue on hover. Description right-aligned in Quiet Steel, wrapping under on narrow widths.

### Status Pill
- Full pill, Strong Hairline border, Muted Steel 14px text, a 7px amber status dot with a soft halo. Used once, for the early-preview notice.

### Requirements List
- Left-aligned list centered as a block, each line led by an 18px green check icon, Muted Steel text.

## Do's and Don'ts

### Do:
- **Do** show the real application, framed as a window with the Window float shadow.
- **Do** keep Selection Blue for download actions and primary list markers only.
- **Do** separate sections and rows with 1px hairlines (9% white; 16% on frames and pills).
- **Do** keep a download button within reach: top bar, hero, and the closing section.
- **Do** honor reduced motion: no smooth scroll, no hero rise.

### Don't:
- **Don't** add cards, panels or shadows to text blocks; only screenshots and buttons float.
- **Don't** use Beam Pink for anything but the beam.
- **Don't** add a second typeface, eyebrow labels or section kickers.
- **Don't** draw mockups or illustrations of the product in place of screenshots.
