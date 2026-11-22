# Beam polarization — design

Date: 2026-11-23 · Status: draft for approval

## Intent

LabSetup is used to **check a layout before building it on the real table**. The beam trace
already carries a Gaussian q-parameter (waist, focus, M²), but it ignores polarization: the PBS
splits 50/50, waveplates do nothing, and a periscope does not rotate the polarization. The point
of this work is to catch polarization mistakes on screen — wrong PBS port, wrong waveplate angle,
the 90° rotation of a periscope — instead of on the table with a power meter.

## Goals

1. Each traced beam segment carries its **polarization** and its **power fraction** (relative to
   its source).
2. Optics act on polarization like ideal real optics: mirror, PBS, BS, half-wave plate (HWP),
   quarter-wave plate (QWP), and **optical (Faraday) isolator**.
3. Polarization is general: linear at any angle, circular left and right, and elliptical in between,
   both from the laser and after any combination of optics.
4. The user can **assign a face's role** (mirror / PBS / BS / HWP / QWP / isolator) on a part,
   including imported parts, through the existing connector wizard.
5. The user can set what they turn on the real table, in the inspector: the **laser's input
   polarization**, each **waveplate's fast-axis angle** (any angle), and each **isolator's
   input-polarizer angle**.
6. The inspector shows, at any optic the beam hits, the **polarization state** and **power
   fraction** there.
7. A **beam profiler probe in Top view**: click a beam and pin a card showing spot size,
   polarization ellipse, power and distance from the laser at that point. It updates live while
   optics are adjusted.

## Non-goals (this round)

- Smooth hyperbolic waist rendering; the IR dashed style stays (DESIGN.md §7).
- Iris clipping, aperture loss, Gaussian clipping at mirrors.
- Non-ideal optics: PBS extinction ratio, BS ratios other than 50/50, waveplate retardance error,
  wavelength-dependent retardance, coating phase shifts on dielectric mirrors.
- Absolute power (mW). Only fractions of the source.
- Drawing polarization in the 3D view (arrows, ellipses, colour by state).
- Assigning face roles per placed part (rejected approach 2 below).
- A standalone linear polarizer or Faraday rotator part (the isolator contains both; split them out
  later if needed). Isolator insertion loss and finite isolation (e.g. −40 dB): ideal only.
- Saving probes in the project; probes in the 3D view; a profile intensity image beyond the drawn
  spot circle.
- The quantum-circuit export (DESIGN.md §10) — this work makes it easier later, but it is not part of it.

## Anti-goals

- A result that looks plausible but is physically wrong (e.g. a periscope that does not rotate
  the polarization, or a handedness that flips the wrong way). A wrong polarization answer is
  worse than none, because the user will trust it on the table.
- Breaking existing projects or user-library parts: old files must load, and user-library parts
  with legacy kinds trace as before. Projects using the **built-in** PBS cubes and waveplates
  will trace differently: they now behave physically (see "Back-compat" below).
- New dependencies (the workspace bans network crates; complex numbers do not need a crate).

## Constraints

- `lab-optics` stays pure and deterministic; the viewport and the app keep calling the same
  `trace_document`.
- Per-node settings live in `Node.params` (`ParamValue::Number` / `Flag` / `Text`) — no schema
  change, as for the existing `beam_*` keys.
- Idle stays near zero: tracing still runs only on document/catalog changes.

## Approaches considered

1. **Extend the connector wizard + inspector angles + 3D field vector in the trace** — *chosen.*
   Face roles are set once per part type (the wizard already does this for imported parts);
   per-placed settings are only the things you physically turn (laser polarization, waveplate
   angle). Reuses existing UI. Trade-off: a single placed mirror can't be told "use the back side"
   on its own.
2. **Per-placed face assignment in the main viewport.** Each node stores its own ports. More
   flexible, but needs face picking in the main view, per-node port storage, and duplicates the
   wizard. Rejected: for layout checking, part-level roles plus per-placed angles cover what is
   adjusted on a real table.
3. **Jones 2-vector in a per-segment s/p basis.** Textbook bookkeeping; same physics, but every
   reflection needs a basis transform and the periscope case is easy to get wrong. Rejected in
   favour of a 3D field vector (an internal choice inside approach 1).

## Design

### Physics model (`lab-optics`)

- **Field:** each ray carries a complex 3D field vector **E = re + i·im** (two `DVec3`),
  perpendicular to the ray direction. Power fraction = |re|² + |im|². No complex crate.
- **Source:** built from two laser params (below). "Horizontal" = perpendicular to the beam and
  to room +Z (parallel to the table); "vertical" = perpendicular to the beam and to horizontal.
  For a beam travelling along ±Z, horizontal falls back to the source port frame's `x`.
- **Mirror** (ideal conductor): **E′ = 2(E·n)n − E** applied to re and im; direction reflects as
  today. This gives the correct handedness flip and the periscope's 90° rotation.
- **PBS:** plane of incidence from direction **d** and port normal **n**; ŝ = normalize(d × n),
  p̂ ⊥ ŝ and d. Transmitted ray keeps only the p component; reflected ray takes the s component
  and then the mirror formula. At normal incidence (d ∥ n) everything transmits.
- **BS (50/50, ideal, polarization-independent):** transmitted = E/√2; reflected = mirror(E)/√2.
- **Waveplate:** retardance Γ = π (HWP) or π/2 (QWP). The fast axis is a fixed physical line
  on the plate, not tied to the beam:
  - Start from **lab horizontal relative to the port normal n** (perpendicular to n and room +Z;
    the port frame `x` if n ∥ Z).
  - Rotate it by `wp_angle_deg` about **n**, using the angle sense below for a beam travelling along +n.
  - Project it onto the plane ⊥ d and normalize to get f̂. Slow axis ŝ = d × f̂.

  So a beam passing back through the plate sees the same physical axis, which the double-pass
  test depends on. A plate placed facing backwards reads its angle mirrored, like a real mount
  turned around. The port frame `x` is deliberately not used (the wizard sets it arbitrarily on
  imported parts).
  **E′ = (E·f̂) f̂ + e^{iΓ}(E·ŝ) ŝ.** The beam direction and q are unchanged.
- **Isolator** (ideal Faraday isolator: input polarizer → 45° Faraday rotator → output polarizer
  at +45°). Angles use the waveplate reference: lab horizontal relative to n, rotated about n,
  angle sense for a beam travelling along +n. Let θ = `iso_angle_deg`.
  - **Forward** (d·n > 0): project E onto the input-polarizer axis at θ (Malus loss), rotate it
    by +45° about n. It leaves at θ+45° and passes the output polarizer.
  - **Backward** (d·n < 0): project E onto the output-polarizer axis at θ+45°, then rotate by +45°
    about **n**. The Faraday effect is non-reciprocal: the rotation sense is fixed by the magnet,
    not by the beam direction. The light reaches the input polarizer at θ+90° and is blocked:
    power 0, so the branch is pruned.
  - Direction and q are unchanged. The isolator's input face is the side n points away from;
    reversing the part reverses which way it passes light, as on the table.
- **Lens, pass:** E unchanged. **Stop:** beam ends (as today).
- **Pruning:** a branch whose power fraction is below **1e-4** is not traced further and gives no
  segment. A well-aligned PBS therefore draws one beam, not two. (`shortcut:` fixed threshold;
  make it a setting if users need to see weak leaks.)
- **Conventions** (written once in a doc comment in `lab-optics`):
  - Time dependence e^{−iωt}. The slow axis gets the phase e^{+iΓ}.
  - Angles (`beam_pol_angle_deg`, `wp_angle_deg`) are measured from lab horizontal, positive
    **counter-clockwise as seen by the receiver** (looking toward the source, horizontal to the
    right and vertical up).
  - Handedness follows Hecht: right-circular rotates clockwise as seen by the receiver; positive
    ellipticity means right-handed.
  - **Anchor case:** horizontal light through a QWP with its fast axis at +45° gives **"Circular R"**.
    This is a test.

### Back-compat

- User-library parts keep their legacy kinds, so they trace as before.
- The built-in PBS cubes, HWP and QWP switch to the new kinds and behave physically. With the
  default horizontal input, a PBS now transmits everything.
- The bundled demo project sets its HWP to `wp_angle_deg = 22.5`, so both detectors are still hit.
  `demo_beam_ends_on_the_detectors` keeps passing.
- No migration of user projects. Opening an old project shows the physical result, which is the point.

### Data

- **New port kinds** (`OpticalPort.kind` strings): `"pbs"`, `"bs"`, `"hwp"`, `"qwp"`, `"isolator"`.
  - `PortKind` gains `Pbs`, `Bs`, `Waveplate { retardance }`, `Isolator` (or equivalent).
  - **New built-in part** `generic:isolator`: "Optical Isolator (Faraday)", category
    `optics/isolators`, one `isolator` port on the body axis at beam height. It reuses an existing
    generator (e.g. `fiber_coupler` or `modulator_box`; no new mesh work this round).
  - Back-compat: legacy `"splitter"` → BS (as today), legacy `"waveplate"` → pass unchanged (as today).
  - Built-in library: PBS cubes → `"pbs"`, BS cubes → `"bs"`, HWP → `"hwp"`, QWP → `"qwp"`.
  - Update the kind list in `lab-core/src/part.rs` (doc comment) and `docs/CATALOG-FORMAT.md`.
- **`Segment` gains**:
  - `pol`: the field at the segment, `re`/`im`;
  - `power: f64`;
  - the q-parameter at `start` (and λM²), so a method `Segment::w_at(point)` gives the exact
    Gaussian radius anywhere along it, which the probe needs.
- **Isolator node param:** `iso_angle_deg`: input-polarizer angle; default 0 (horizontal).
- **Laser node params** (numbers, alongside `beam_wavelength_nm` etc.):
  - `beam_pol_angle_deg`: azimuth of the polarization ellipse from horizontal; default 0.
  - `beam_pol_ellipticity_deg`: 0 = linear, +45 = right circular, −45 = left circular; default 0.
- **Waveplate node param:** `wp_angle_deg`: fast axis from lab horizontal relative to the port
  normal, rotated about the port normal (see Waveplate above); default 0.

### UI

- **Connector wizard** (`lab-app/src/wizard.rs`): optic kinds become
  `mirror, lens, bs, pbs, hwp, qwp, isolator, source, detector, dump, fiber`, with readable
  labels ("Beamsplitter (50/50)", "Polarizing beamsplitter", "Half-wave plate",
  "Quarter-wave plate", "Optical isolator"). For an isolator, the clicked face's normal is the
  pass direction.
- **Inspector Beam section** (`lab-app/src/beams.rs` + its Slint section):
  - Laser: two more fields, "Polarization angle" (°) and "Ellipticity" (°), next to wavelength, waist and M².
  - Waveplate (a part with an `hwp`/`qwp` port): one field, "Fast axis" (°). It accepts any angle
    (wrapped to 0–180°) and retraces on commit, so the probes update.
  - Isolator: one field, "Input polarizer" (°).
  - Laser presets: the polarization fields are plain numbers, but common states are one entry away
    (angle 0 / 90 / 45 with ellipticity 0 for linear H / V / D; ellipticity ±45 for circular R/L).
    Whether a preset dropdown is worth adding is left to the builder.
  - Any optic hit by a beam: **one line per hit**, in trace order, because double passes
    (QWP → mirror → back through the PBS) are exactly where mistakes hide. For example:
    `Pass 1 · Beam Ø 1.02 mm · 312 mm from source · Linear 45° · 50 % power`.
    - With more than one laser in the scene, each line starts with the laser's name.
    - The single beam text becomes a short list in the Slint model.
    - `Trace::radius_at` stays as it is (first hit) for other callers.
  - The state reads "Linear θ°", "Circular R/L" (|ellipticity| > 44.5°), or "Elliptical θ°, ε°".
- **Live control / CLI:** the new params are accepted by `live set-params` (extend the
  `live_set_params_knows_every_beam_param` test), and the `beams` output includes `power` and the
  state per segment.

### Beam profiler probe (Top view)

```
Top view                                   Probe card
  laser ──────●────[HWP]───[PBS]──── D1    ┌──────────────────────┐
              ▲ probe pinned here          │  ◯ 2w = 1.02 mm      │  spot circle, to scale in the card
                                           │  ⬭↻ Linear 22°       │  polarization ellipse + handedness
                                           │  ▮▮▮▯ 62 % power     │
                                           │  312 mm from Laser 1 │
                                           └──────────────────────┘
```

- **Tool:** "Beam Profiler", an action in the command palette and the Top view toolbar, next to
  Measure. It follows the Measure tool's pattern (`lab-viewport/src/measure.rs`): a viewport tool
  state plus a request/event pair with the app.
- **Pinning:** in Top view, with the tool active, a click within a few pixels of a drawn beam pins
  a probe at the nearest point on that segment (projected in the top-down view; the probe keeps
  the segment's height). A click away from any beam does nothing. Up to **4** probes; a 5th
  replaces the oldest. Each card has a ×, and Esc with the tool active clears all.
- **Stored as a world point, not a segment index.** Segment indices change on every retrace. After
  each retrace, each probe re-snaps to the nearest segment within a small tolerance (≈ 2 mm). If
  none is near, the card says "No beam here" and stays, so moving a mirror away and back does not
  lose it.
- **Overlapping passes:** when several segments run through the probe point (double pass, or two
  lasers on one path), the card lists each, like the inspector lines.
- **Card contents** per pass:
  - the spot as a circle of diameter 2w(z), using the exact Gaussian w from `Segment::w_at`;
  - the polarization ellipse, with an arrow for handedness;
  - the state text, using the inspector's wording;
  - the power fraction as % with a small bar;
  - the distance from the laser, with the laser's name.

  The ellipse is drawn as seen by the receiver (horizontal right, vertical up), the same frame as
  the angle convention.
- **Live:** the cards recompute on every retrace (document change, waveplate field commit, gizmo
  drag preview), with no timer.
- **Lifetime:** probes are a view aid. They are not saved and not undoable. They show only in Top
  view and are kept in memory (hidden) when switching to 3D.
- **Where it's drawn:** as a Slint overlay card next to the projected probe point, as the Measure
  readout is drawn. The exact card layout is left to the builder.

### Verification (unit tests in `lab-optics`)

1. Horizontal into a PBS → ~100 % transmitted, no reflected segment; vertical → ~100 % reflected.
2. HWP at 22.5° on horizontal → 50 % / 50 % at a following PBS.
3. QWP at 45° on horizontal → circular (|ellipticity| ≈ 45°), then 50/50 at a PBS.
4. Two QWPs at 45° act like one HWP at 45° (horizontal → vertical).
5. Periscope (two mirrors as on `thorlabs-RS99`, beam turned 90° in azimuth) → horizontal becomes
   vertical.
6. A 45° in-plane fold mirror keeps horizontal horizontal.
7. BS → two segments at 50 % each; power is conserved at every PBS/BS split.
8. Legacy `"splitter"` / `"waveplate"` parts trace as before.
9. Anchor: horizontal + QWP at +45° → right-circular (sign of the ellipticity, not only |ε|).
10. Double pass: horizontal → QWP at 45° → mirror at normal incidence → back through the QWP →
    vertical, so a PBS on the way back reflects it (the classic optical isolator).
11. The demo project still ends on both detectors with its HWP at 22.5°.
12. Isolator forward: horizontal into an isolator at θ = 0 → 100 % power out, linear at +45°.
    Input at 45° to the polarizer → 50 %.
13. Isolator backward: the forward output retro-reflected by a mirror at normal incidence → 0 %
    out of the input side (branch pruned). Light entering the output side at any polarization →
    0 % out.
14. Isolator reversed in place (part rotated 180°) blocks the forward beam.
15. `Segment::w_at` matches the analytic w(z) at the waist and at one Rayleigh range (w0·√2).
16. Probe snapping (app/viewport logic, unit-testable without a GPU): a point 1 mm off a segment
    snaps to it; a point after retrace with the segment moved 50 mm away reports "No beam here".

## Left to the builder

- Exact struct shapes (`Segment.pol` representation, `PortKind` variant names).
- Slint layout details of the new fields and the wizard labels.
- Whether the pruning threshold is a `const` in `lab-optics` (expected) and its exact value
  around 1e-4.
- How the `beams` live output formats the polarization state.
