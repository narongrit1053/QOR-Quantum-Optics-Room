# Beam Polarization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry polarization and power through the beam trace (mirror, PBS, BS, HWP, QWP, Faraday isolator), expose the knobs in the inspector and wizard, and add a Top-view beam-profiler probe.

**Architecture:** All physics lives in `lab-optics` (pure): each ray carries a complex 3D field `Field { re, im }`, optics are linear maps on it, and `Trace::probe` answers "what is the beam at this point". The app reads the trace for the inspector and the probe cards; the viewport only pins probe points (screen-space snap) and projects them through the existing Top-view overlay channel.

**Tech Stack:** Rust, glam `DVec3`, Bevy 0.20 (viewport), Slint (UI). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-23-beam-polarization-design.md`

## Global Constraints

- `lab-optics` stays pure and deterministic; no Bevy, no new crates (complex numbers are two `DVec3`).
- Per-node settings live in `Node.params` as `ParamValue::Number`: `beam_pol_angle_deg` (0), `beam_pol_ellipticity_deg` (0), `wp_angle_deg` (0), `iso_angle_deg` (0). No schema change.
- New port kind strings: `"pbs"`, `"bs"`, `"hwp"`, `"qwp"`, `"isolator"`. Legacy `"splitter"` → BS, legacy `"waveplate"` → pass unchanged.
- Conventions (doc comment in `lab-optics`, verbatim from spec): time dependence e^{−iωt}; slow axis gets e^{+iΓ}; angles from lab horizontal, positive counter-clockwise as seen by the receiver (looking toward the source, horizontal right, vertical up); Hecht handedness, positive ellipticity = right-handed. Anchor: horizontal + QWP fast axis +45° → "Circular R".
- Pruning: a branch with power fraction < `1e-4` is not traced further.
- Probes: max 4, re-snap tolerance ≈ 2 mm, not saved, not undoable, Top view only.
- Idle stays near zero: no timers; everything recomputes on retrace/overlay events only.
- Build env (Bash): `export CARGO_HOME=/d/dev/cargo RUSTUP_HOME=/d/dev/rustup PATH=/d/dev/cargo/bin:$PATH`. App builds are slow on first run; run long cargo commands in the background.

## Review Focus

1. **Beam travelling along ±Z** (periscope's vertical leg): "horizontal" is undefined there; must fall back to the port frame `x` with no NaN. → Task 1 test `vertical_beam_has_finite_field`.
2. **Waveplate hit at oblique incidence or from its back side**: the fast axis is projected; power must be conserved and finite. → Task 2 test `waveplate_back_side_and_oblique_conserve_power`.
3. **Probe exactly at a joint between two consecutive segments** (at a mirror): must report one pass, not two. → Task 4 test `probe_at_a_joint_is_one_pass`.
4. **Angle fields with out-of-range values** (−30, 400): wrapped, not rejected; non-numbers stay rejected by the existing field path. → Task 6 test `angle_fields_wrap`.
5. **Probe pinned and then the laser turned off / beam gone**: card says "No beam here", no panic. → Task 4 test `probe_on_empty_trace_is_empty` and Task 7 test `card_without_readings_says_no_beam`.

---

### Task 1: Field type, conventions, mirror polarization

**Files:**
- Create: `crates/lab-optics/src/pol.rs`
- Modify: `crates/lab-optics/src/lib.rs` (`Source`, `Segment`, `Ray`, `trace`, `scene`, tests' `source()` helper)
- Modify: `crates/lab-viewport/src/beams.rs` (test that builds a `Segment` literal: add the new fields)

**Interfaces:**
- Produces (in `lab_optics`, re-exported from `pol`):
  - `pub struct Field { pub re: DVec3, pub im: DVec3 }` with `fn power(&self) -> f64`, `fn scale(self, k: f64) -> Field`, `fn map(self, f: impl Fn(DVec3) -> DVec3) -> Field` (apply a real-linear map to re and im), `fn dot(&self, a: DVec3) -> (f64, f64)` (complex E·a).
  - `pub struct Polarization { pub azimuth_deg: f64, pub ellipticity_deg: f64 }` with `fn label(&self) -> String`.
  - `pub fn horizontal(dir: DVec3, fallback_x: DVec3) -> DVec3`
  - `pub fn field_from(p: Polarization, dir: DVec3, h: DVec3) -> Field` (unit power)
  - `pub fn polarization_of(f: &Field, dir: DVec3, h: DVec3) -> Polarization`
  - `pub fn mirror(f: Field, n: DVec3) -> Field`
  - `Source` gains `pub pol: Polarization`, `pub frame_x: DVec3`.
  - `Segment` gains `pub field: Field`, `pub power: f64`, `pub q_start: (f64, f64)`, `pub lm2: f64`, and a `pub frame_x: DVec3` (the horizontal fallback for this ray, carried from the source).

- [ ] **Step 1: Write the failing tests** (in `lab-optics` tests module)

```rust
#[test] fn source_state_round_trips() {
    for (a, e) in [(0.0, 0.0), (90.0, 0.0), (45.0, 0.0), (30.0, 20.0), (0.0, 45.0), (0.0, -45.0)] {
        let h = horizontal(DVec3::Y, DVec3::X);
        let p = polarization_of(&field_from(Polarization { azimuth_deg: a, ellipticity_deg: e }, DVec3::Y, h), DVec3::Y, h);
        assert!((p.ellipticity_deg - e).abs() < 1e-9);
        if e.abs() < 44.9 { assert!((p.azimuth_deg - a).abs() < 1e-9, "{a} {e} -> {p:?}"); }
    }
}
#[test] fn labels() {
    assert_eq!(Polarization { azimuth_deg: 45.0, ellipticity_deg: 0.0 }.label(), "Linear 45°");
    assert_eq!(Polarization { azimuth_deg: 0.0, ellipticity_deg: 45.0 }.label(), "Circular R");
    assert_eq!(Polarization { azimuth_deg: 0.0, ellipticity_deg: -44.8 }.label(), "Circular L");
    assert_eq!(Polarization { azimuth_deg: 30.0, ellipticity_deg: 12.0 }.label(), "Elliptical 30°, 12°");
}
#[test] fn fold_mirror_keeps_horizontal() // source along +Y, H; 45° mirror in the table plane → reflected segment reads azimuth 0 (mod 180), ellipticity 0, power 1
#[test] fn periscope_rotates_horizontal_to_vertical() // mirrors as thorlabs-RS99 (lower z=[0,-0.7071,0.7071] at z=50, upper z=[0,0.7071,-0.7071] at z=140) but the upper rotated so the top leg runs along +X; H in → last segment azimuth 90 ±1e-6
#[test] fn vertical_beam_has_finite_field() // the periscope's vertical segment: field and polarization_of are finite (no NaN)
```

- [ ] **Step 2: Run** `cargo test -p lab-optics --offline` — Expected: FAIL (unresolved `Field`, `Polarization`, …).

- [ ] **Step 3: Implement `pol.rs` and wire it into `trace`**

  - `horizontal(d, fx)`: `Z.cross(d)` normalized; if its length < 1e-9, project `fx` onto the plane ⊥ d and normalize. Vertical `v = d.cross(h)`.
  - Jones in (h, v): `field_from` uses `Ex = cosψ cosχ + i sinψ sinχ`, `Ey = sinψ cosχ − i cosψ sinχ` (this gives R for χ = +45° under the conventions).
  - `polarization_of`: Stokes from `Ex = f.dot(h)`, `Ey = f.dot(v)`: `S1 = |Ex|²−|Ey|²`, `S2 = 2 Re(Ex conj Ey)`, `S3 = 2 Im(Ex conj Ey)`; `ψ = ½ atan2(S2, S1)` wrapped to [0,180), `χ = ½ asin(S3/S0)`.
  - `label`: `|χ| > 44.5` → "Circular R/L" (sign); `|χ| < 0.5` → "Linear {ψ:.0}°"; else "Elliptical {ψ:.0}°, {χ:.0}°".
  - `mirror(f, n) = f.map(|e| 2.0 * e.dot(n) * n - e)`.
  - `Ray` gains `field: Field`, `frame_x: DVec3`; the source ray starts at `field_from(s.pol, dir, horizontal(dir, s.frame_x))`. Mirror ports apply `mirror`; every segment records `field`, `power = field.power()`, `q_start = (beam.re, beam.im)`, `lm2`.
  - `scene`: `pol` from `beam_pol_angle_deg` / `beam_pol_ellipticity_deg` (default 0/0), `frame_x = pose.transform_vector3(op.frame.x)`.
  - Put the conventions block (Global Constraints) as the `pol.rs` module doc.

- [ ] **Step 4: Run** `cargo test -p lab-optics --offline` — Expected: PASS (old tests included).

- [ ] **Step 5: Commit** — `git add crates/lab-optics crates/lab-viewport/src/beams.rs && git commit -m "lab-optics: carry a polarization field; mirrors act on it"`

---

### Task 2: PBS, BS, waveplates, pruning, port kinds

**Files:**
- Modify: `crates/lab-optics/src/lib.rs` (`PortKind`, `Port`, `trace`, `scene`), `crates/lab-optics/src/pol.rs`

**Interfaces:**
- Consumes: Task 1 `Field`, `mirror`, `horizontal`.
- Produces:
  - `PortKind::{Source, Mirror, Lens, Bs, Pbs, Hwp, Qwp, Isolator, Pass, Stop}` (`Splitter` renamed `Bs`; `from_kind`: `"splitter" | "bs"` → `Bs`, `"pbs"` → `Pbs`, `"hwp"` → `Hwp`, `"qwp"` → `Qwp`, `"isolator"` → `Isolator`, legacy `"waveplate"` → `Pass`).
  - `Port` gains `pub angle_deg: f64` (from `wp_angle_deg` or `iso_angle_deg` on the node, default 0) and `pub frame_x: DVec3`.
  - `pub const MIN_POWER: f64 = 1e-4;`
  - `pub fn plate_axis(n: DVec3, frame_x: DVec3, angle_deg: f64, d: DVec3) -> DVec3` — fixed physical axis on a plate, projected ⊥ d (shared with Task 3).
  - `pub fn waveplate(f: Field, d: DVec3, fast: DVec3, retardance: f64) -> Field`
  - `pub fn pbs(f: Field, d: DVec3, n: DVec3) -> (Field, Field)` — (transmitted, reflected-before-mirror)

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn pbs_sorts_h_and_v() // H into a PBS (port normal (0.7071,0.7071,0)) → one segment after it, power ≈1, transmitted; V → one reflected segment, power ≈1
#[test] fn hwp_22_5_gives_50_50() // H → HWP angle 22.5 → PBS: two segments after the PBS, powers 0.5 ± 1e-9
#[test] fn qwp_45_anchor_is_right_circular() // H → QWP angle +45: segment after it has ellipticity_deg ≈ +45 (sign checked), label "Circular R"; then a PBS gives 0.5/0.5
#[test] fn two_qwp_make_a_hwp() // H → QWP 45 → QWP 45 → azimuth 90, ellipticity 0
#[test] fn bs_splits_50_50_and_conserves_power() // BS: two segments, 0.5 each; sum of powers at every split == parent power (1e-12)
#[test] fn double_pass_isolates() // H → QWP 45 → mirror at normal incidence → back through the QWP → PBS reflects it: last segment's polarization azimuth 90 and it leaves the PBS on the reflected side
#[test] fn legacy_kinds_trace_as_before() // a "splitter" port behaves as BS (2 segments, depth 1), a "waveplate" port passes the field unchanged
#[test] fn waveplate_back_side_and_oblique_conserve_power() // beam hits a QWP along -n, and at 30° incidence: power stays 1 ±1e-12, field finite
```
Also update the existing `splitter_emits_two_beams_and_bounces_are_capped` helper to the new `Port` fields.

- [ ] **Step 2: Run** `cargo test -p lab-optics --offline` — Expected: FAIL.

- [ ] **Step 3: Implement**
  - `plate_axis`: `hn = horizontal(n, frame_x)`; `f0 = cosθ·hn + sinθ·n.cross(hn)`; return `normalize(f0 − f0.dot(d)·d)`.
  - `waveplate`: `s = d.cross(fast)`; `E′ = (E·f) f + e^{iΓ}(E·s) s` with complex coefficients from `Field::dot`; Γ = π (Hwp) or π/2 (Qwp).
  - `pbs`: `ŝ = normalize(d × n)`; if `|d × n| < 1e-9` everything transmits. Reflected part = s-component, transmitted = the rest; the reflected ray then goes through `mirror`.
  - BS: both rays carry `field.scale(FRAC_1_SQRT_2)`, reflected through `mirror`.
  - In `trace`, never push a ray with `field.power() < MIN_POWER`. Keep traversal order (transmitted traced first).
  - `scene`: fill `angle_deg` from `wp_angle_deg` for Hwp/Qwp ports and `iso_angle_deg` for Isolator ports; `Isolator` behaves as `Pass` until Task 3.

- [ ] **Step 4: Run** `cargo test -p lab-optics --offline` — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -am "lab-optics: PBS, BS, waveplates, power pruning"`

---

### Task 3: Faraday isolator

**Files:**
- Modify: `crates/lab-optics/src/pol.rs`, `crates/lab-optics/src/lib.rs`

**Interfaces:**
- Consumes: `plate_axis`, `Port::angle_deg`.
- Produces: `pub fn isolator(f: Field, d: DVec3, n: DVec3, frame_x: DVec3, angle_deg: f64) -> Field`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn isolator_forward_passes_and_rotates() // H along +n, θ=0 → power 1, azimuth 45 (receiver frame of the outgoing beam)
#[test] fn isolator_forward_malus() // input at 45° to the polarizer → power 0.5
#[test] fn isolator_blocks_the_return() // isolator then a mirror at normal incidence: the returning ray is pruned at the isolator (no segment leaves its input side)
#[test] fn isolator_blocks_any_backward_input() // beam entering along -n with H, V, 45°, circular → power after < MIN_POWER
#[test] fn reversed_isolator_blocks_forward() // same scene with the port normal flipped → forward beam pruned
```

- [ ] **Step 2: Run** `cargo test -p lab-optics --offline` — Expected: FAIL.

- [ ] **Step 3: Implement `isolator`**
  - `a = plate_axis(n, fx, θ, d)`, `b = plate_axis(n, fx, θ + 45, d)`; `P_x(E) = (E·x) x`; `R(E) = E.map(|v| v·cos45° + n.cross(v)·sin45°)` — the rotation is about **n**, never about d (non-reciprocal).
  - Forward (`d·n > 0`): `P_b(R(P_a(E)))`. Backward: `P_a(R(P_b(E)))`. Direction and q unchanged; pruning removes the blocked branch.
  - Wire `PortKind::Isolator` in `trace`.

- [ ] **Step 4: Run** `cargo test -p lab-optics --offline` — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -am "lab-optics: ideal Faraday isolator"`

---

### Task 4: Probe API (exact w, path length, readings)

**Files:**
- Modify: `crates/lab-optics/src/lib.rs` (or a new `crates/lab-optics/src/probe.rs` if `lib.rs` passes ~600 lines)
- Modify: `crates/lab-app/src/beams.rs` (delete its private `path_length`; call the new one)

**Interfaces:**
- Produces:
  - `impl Segment { pub fn w_at(&self, p: DVec3) -> f64 }` — exact Gaussian radius from `q_start + distance(start, p)` and `lm2`.
  - `impl Trace { pub fn path_length(&self, i: usize) -> f64 }` — moved from `lab-app/src/beams.rs`, same algorithm.
  - `pub struct ProbeReading { pub source: NodeId, pub segment: usize, pub w: f64, pub pol: Polarization, pub power: f64, pub distance: f64 }`
  - `impl Trace { pub fn probe(&self, at: DVec3, tol: f64) -> Vec<ProbeReading> }` — every segment whose closest point to `at` is within `tol`, using the half-open parameter range `[0, len)` so a joint counts once; `distance` = path length to the start of the segment + distance along it; `pol` uses `horizontal(dir, segment.frame_x)`.
  - `pub const PROBE_TOL_MM: f64 = 2.0;`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn w_at_matches_analytic() // collimated 0.5 mm waist → lens f=100: w_at(waist point) == waist w (1e-9); at waist + zR: w0·√2 (1e-6)
#[test] fn probe_reads_power_and_state() // H → HWP 22.5 → PBS; probe 50 mm after the PBS on the transmitted arm → 1 reading, power 0.5, "Linear 0°", distance == path to that point
#[test] fn probe_at_a_joint_is_one_pass() // probe exactly at a fold-mirror hit point → 1 reading (the outgoing segment)
#[test] fn probe_lists_a_double_pass() // the isolator/QWP double-pass scene: probe between QWP and mirror → 2 readings in trace order
#[test] fn probe_off_beam_and_on_empty_trace_is_empty() // point 1 mm off a segment → 1 reading; 50 mm off → 0; Trace::default() → 0
```

- [ ] **Step 2: Run** `cargo test -p lab-optics --offline` — Expected: FAIL.

- [ ] **Step 3: Implement** as in Interfaces.

- [ ] **Step 4: Run** `cargo test -p lab-optics --offline` then `cargo test -p lab-app --offline beams` — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -am "lab-optics: probe readings and exact beam radius"`

---

### Task 5: Library parts, wizard roles, demo, docs

**Files:**
- Modify: `library/parts/optics/cubes/generic-pbs-cube-{0.5in,1in}.toml` (kind `"pbs"`), `generic-bs-cube-{0.5in,1in}.toml` (kind `"bs"`), `library/parts/optics/waveplates/generic-hwp-1in.toml` (`"hwp"`), `generic-qwp-1in.toml` (`"qwp"`)
- Create: `library/parts/optics/isolators/generic-isolator.toml`
- Modify: `crates/lab-app/src/wizard.rs:20` (`OPTIC_KINDS` and the label model sent to `set_wizard_optic_kinds`)
- Modify: `crates/lab-core/src/sample.rs` (HWP node gets `wp_angle_deg = 22.5`)
- Modify: `crates/lab-core/src/part.rs:137` (kind list comment), `docs/CATALOG-FORMAT.md` (kinds + node params)

**Interfaces:**
- Produces: catalog id `generic:isolator`, name `Optical Isolator (Faraday)`, category `optics/isolators`, one port `name = "isolator"`, `kind = "isolator"`, normal +Y = pass direction, at the generator's `axis_height`; geometry reuses generator `fiber_coupler` (or `modulator_box`), with the `optic_bore`/seat connector copied from a sibling part of the same generator.
- `OPTIC_KINDS = ["mirror", "lens", "bs", "pbs", "hwp", "qwp", "isolator", "source", "detector", "dump", "fiber"]`; labels shown: "Mirror", "Lens", "Beamsplitter (50/50)", "Polarizing beamsplitter", "Half-wave plate", "Quarter-wave plate", "Optical isolator", "Laser output", "Detector", "Beam dump", "Fiber".

- [ ] **Step 1: Write the failing tests**
  - In `lab-catalog` (where built-in TOML parse tests live): `generic:isolator` parses and its port kind maps to `PortKind::Isolator` (assert the string `"isolator"` there; the mapping is covered in lab-optics).
  - In `lab-app/src/beams.rs`: `demo_beam_ends_on_the_detectors` must keep passing after the PBS turns polarizing (it fails between the TOML change and the sample change — run it after Step 3a to see that).
  - In `lab-app/src/wizard.rs` tests (or a new one): `OPTIC_KINDS` contains every new kind and `OPTIC_LABELS.len() == OPTIC_KINDS.len()`.

- [ ] **Step 2: Run** `cargo test -p lab-catalog -p lab-app --offline` — Expected: FAIL (isolator missing; demo test fails once the PBS kind changes).

- [ ] **Step 3: Implement** the TOML edits (3a), then the sample HWP param, the wizard kinds/labels, the docs.

- [ ] **Step 4: Run** `cargo test -p lab-catalog -p lab-core -p lab-app --offline` — Expected: PASS, including `demo_beam_ends_on_the_detectors` and `demo_mirror_rotation_moves_the_beam`.

- [ ] **Step 5: Commit** — `git commit -am "Library: PBS/BS/HWP/QWP kinds, Faraday isolator part; wizard roles; demo HWP at 22.5°"`

---

### Task 6: Inspector fields, per-hit lines, live control

**Files:**
- Modify: `crates/lab-app/src/beams.rs` (`PARAMS`, `fill`, `describe`, `field_edit`)
- Modify: `crates/lab-app/src/live/read.rs:134` (`Beams` output), the live set-params key list that `live_set_params_knows_every_beam_param` checks
- Modify: `crates/lab-ui/ui/inspector.slint:274-302` only if the Beam section header must also show for waveplates/isolators (it shows when `beam-fields` is non-empty — add that condition)

**Interfaces:**
- Consumes: `Trace::path_length`, `Segment.field/power`, `polarization_of`, `horizontal`, `PortKind`.
- Produces:
  - Laser `PARAMS` gains `("beam_pol_angle_deg", "Polarization angle", "°", 0.0)`, `("beam_pol_ellipticity_deg", "Ellipticity", "°", 0.0)`.
  - `const WAVEPLATE_PARAMS = [("wp_angle_deg", "Fast axis", "°", 0.0)]`, `const ISOLATOR_PARAMS = [("iso_angle_deg", "Input polarizer", "°", 0.0)]`, chosen by the node's port kinds (laser wins if a part has several).
  - `fn describe(trace, id, app) -> String` returns one line per hit joined by `'\n'`: `Pass {k} · Beam Ø {2w:.2} mm · {distance} from source · {pol.label()} · {power*100:.0} % power`; prefixed with the laser node's name when the document has more than one laser. "No beam" when none. (Plan call: a newline-joined string in the existing `beam-text` instead of a new Slint list model — same result, less code.)
  - Angle wrap on commit: `wp_angle_deg`, `iso_angle_deg`, `beam_pol_angle_deg` → `rem_euclid(180.0)`; `beam_pol_ellipticity_deg` clamped to [−45, 45].
  - Live `beams` output: each segment adds `"power": f64`, `"polarization": label`, `"azimuth_deg"`, `"ellipticity_deg"`.

- [ ] **Step 1: Write the failing tests** (in `lab-app/src/beams.rs`)

```rust
#[test] fn demo_inspector_shows_state_and_power() // demo doc: describe(PBS node) contains "Linear" and "% power"; describe(D1) power line shows 50 %
#[test] fn double_pass_lists_two_lines() // QWP-mirror-PBS double pass scene built like the existing tests: describe(QWP) has 2 lines starting "Pass 1", "Pass 2"
#[test] fn angle_fields_wrap() // field_edit("beam.wp_angle_deg", "400") stores 40; "-30" → 150; "abc" is rejected as today; ellipticity "60" → 45
```
And extend `live_set_params_knows_every_beam_param` with the four new keys.

- [ ] **Step 2: Run** `cargo test -p lab-app --offline beams` — Expected: FAIL.

- [ ] **Step 3: Implement** as in Interfaces.

- [ ] **Step 4: Run** `cargo test -p lab-app --offline` — Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -am "Inspector: polarization fields, per-pass beam lines; live beams output"`

---

### Task 7: Top-view Beam Profiler probe

**Files:**
- Create: `crates/lab-viewport/src/probe.rs`
- Modify: `crates/lab-viewport/src/lib.rs` (`Tool::Probe`, `FrameRequest::ClearProbes(Option<usize>)` + the public `ViewportRequest` twin, `OverlayItem::Probe`), `crates/lab-viewport/src/host.rs:233` (add `ProbePlugin`), `crates/lab-viewport/src/beams.rs` (keep the last `Trace` in `Beams` as `pub(crate) trace`), `crates/lab-viewport/src/overlay.rs` (emit probe items)
- Create: `crates/lab-app/src/probe.rs`
- Modify: `crates/lab-app/src/main.rs` (mod), `crates/lab-app/src/actions.rs:64` (action `tool.probe`, "Beam Profiler", "Tools", keywords "beam profiler probe polarization power"), `crates/lab-app/src/controller.rs:609-625` (tool index 6, hint "Click a beam to pin a probe"; `probe.close.<i>` → `ClearProbes(Some(i))`; call `probe::refresh` after `beams::retrace`), `crates/lab-app/src/overlay.rs` (route `OverlayItem::Probe` to `probe::on_items`)
- Modify: `crates/lab-ui/ui/types.slint` (`ProbeCard`), `crates/lab-ui/ui/app.slint` (property `probe-cards`, the card component in the overlay layer, a toolbar button next to Measure at line ~1485, Esc with `tool == 6` → `root.action("probe.clear")`)

**Interfaces:**
- Consumes: `Trace::probe`, `PROBE_TOL_MM`, `ProbeReading`, `Polarization::label`.
- Produces:
  - Viewport: `pub(crate) fn snap_to_beam(cursor: Vec2, segments: &[(Vec2, Vec2, DVec3, DVec3)], max_px: f32) -> Option<DVec3>` — nearest point on the projected 2D segments within `max_px` (12 px, as `SNAP_PX` in `measure.rs`), returned as the lerped world point.
  - `OverlayItem::Probe { index: usize, at: DVec3, pos: Vec2 }` sent with the other Top-view items (so adding/removing a probe or moving the camera sends a new overlay).
  - App: `pub fn on_items(ui, app, items: Vec<(usize, DVec3, Vec2)>)`, `pub fn refresh(ui, app)`; pure `fn card(readings: &[ProbeReading], doc: &Document, unit: LengthUnit) -> (String, Vec<String>)` (title, lines) and `fn ellipse_path(pol: Polarization, radius_px: f32) -> String` (SVG path commands, as `overlay.rs` builds dimension lines).
  - Slint `ProbeCard { index: int, x: length, y: length, title: string, lines: string, spot: length, ellipse: string, power: float }`. Card: spot circle (diameter = `spot`, scaled so the largest 2w among cards fills 40 px; label shows the real 2w), ellipse path with an arrowhead for handedness, power bar, lines, × button → `action("probe.close.<index>")`.
- Behaviour (from spec): pinning only with `Tool::Probe` in Top view; up to 4 probes, a 5th replaces the oldest; points stored in world space; `ClearProbes(None)` on Esc; probes kept (hidden) outside Top view because overlay items are only sent for Top. Plan call: cards update after the app retraces (commands, field commits, gizmo drag end); during a gizmo drag the viewport beam moves but cards update on release — say so in the hint text if it confuses.

- [ ] **Step 1: Write the failing tests**

```rust
// lab-viewport/src/probe.rs
#[test] fn snaps_within_12_px() // cursor 5 px from a projected segment → Some(world point at the same fraction); 20 px → None; picks the nearer of two segments
#[test] fn fifth_probe_replaces_oldest() // push 5 points into the probe list helper → len 4, first is the 2nd pushed
// lab-app/src/probe.rs
#[test] fn card_formats_readings() // reading {w 0.51, power 0.62, pol Linear 22°} → lines contain "2w = 1.02 mm", "Linear 22°", "62 % power", "from Laser"
#[test] fn card_without_readings_says_no_beam() // [] → lines == ["No beam here"]
#[test] fn ellipse_path_is_closed_and_circular_for_circular() // circular → path bbox square (±0.5 px); linear → degenerate line
```

- [ ] **Step 2: Run** `cargo test -p lab-viewport -p lab-app --offline probe` — Expected: FAIL.

- [ ] **Step 3: Implement** viewport side (tool, click → `snap_to_beam` over `Beams.trace` projected with the Top camera like `overlay.rs`'s `project`, probe list, overlay items, `ClearProbes`), then app side and Slint.

- [ ] **Step 4: Run** `cargo test -p lab-viewport -p lab-app --offline` then `cargo build --package lab-app --offline` — Expected: PASS / builds.

- [ ] **Step 5: Commit** — `git commit -am "Top view: Beam Profiler probe with spot, polarization and power"`

---

### Task 8: Whole-feature check

- [ ] **Step 1:** `cargo build --workspace --offline` and `cargo test -p lab-optics -p lab-catalog -p lab-core -p lab-viewport -p lab-app --offline` — Expected: all PASS.
- [ ] **Step 2:** Run the app (the `run` skill; scratch `APPDATA`/`LOCALAPPDATA` as in the dev-environment notes). Open the demo, switch to Top view, pick Beam Profiler, pin a probe after the PBS on each arm: both read ~50 %. Set the HWP "Fast axis" to 0: one probe reads 100 %, the other card says "No beam here". Add a Faraday isolator from the library before the PBS; retro-reflect with a mirror; confirm no beam comes back. Capture one screenshot per check.
- [ ] **Step 3:** Commit any fix-ups; note anything unverified in the hand-off.
