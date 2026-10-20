# M3 plan: Views

Milestone M3 of DESIGN.md §11, built under [agents/FIRST-DRAFT.md](agents/FIRST-DRAFT.md): every task is built once, checked once (`cargo build --package lab-app --offline` plus its one happy-path check), and other flaws go to the Defect log. Read this file and your own section; open only the files your task touches.

**Exit criterion (§11):** switch 3D↔Top smoothly; isolate one plate.

## What M1/M2 already provide (do not rebuild)

- `ViewMode { Perspective, Top }` with an orthographic top camera, eased camera moves (`camera.rs`, `EASE_RATE`, `animate_camera`) and the 3D/Top segmented control (`app.slint` `view-mode`, `controller.rs`).
- `FrameTarget { All, Selection, Platform, Room }`; **F** frames the selection, **Shift+F** frames all (`app.slint:401-405`).
- Hover tint and selection tint by material colour mix (`picking.rs` `apply_highlight`), `HoverChanged`/`SelectionChanged` events, `PickId` on every pickable mesh (`scene.rs:397`).
- Procedural hole shader that stays crisp in the top view down to 0.05 m (`hole_material.rs`, `hole_grid.wesl`).
- `Tool { Select, Move, Rotate }` and the toolbar buttons (POLA-45).

## Stages and owners

| Stage | Issue | Task | Engineer | Owns (new or changed) |
|---|---|---|---|---|
| 2 | POLA-49 | Top view: blueprint look + plate framing | Engineer Opus Low (was Opus) | `lab-viewport/src/blueprint.rs` (new), blueprint params in `hole_material.rs` + `hole_grid.wesl`, `SetBlueprint` in `lib.rs`, blueprint toggle in `app.slint` toolbar + `controller.rs` |
| 2 | POLA-50 | Selection outline + part hover | Engineer Opus Low | `lab-viewport/src/outline.rs` (new), `picking.rs` |
| 2 | POLA-52 | Timed view transitions + isolate plate (moved from stage 3) | Engineer Opus Low | `camera.rs` (animation only), `lab-viewport/src/isolate.rs` (new), `Isolate` in `lib.rs`, isolate UI (scene-row action, inspector button, banner) in `app.slint`/`inspector.slint`, `lab-app/src/scene.rs`, `edit.rs` platform part |
| 2 | POLA-53 | Measure tool (moved from stage 3) | Engineer Opus Low | `lab-viewport/src/measure.rs` (new), `Tool::Measure` + `Measured` in `lib.rs`, measure button + readout in `app.slint`, `lab-app/src/measure.rs` (new) |
| 3 | POLA-51 | Split view | Engineer Opus | `ViewMode::Split`, second camera in `camera.rs`, camera-under-pointer in `picking.rs`/`place.rs`/`gizmo.rs`/`measure.rs`, `host.rs` viewport rects, segmented control item |
| 4 | POLA-54 | Top-view labels + dimensions overlay | Engineer Opus Low | `lab-viewport/src/overlay.rs` (new), `Overlay` event in `lib.rs`, overlay layer in `lab-ui/ui/overlays.slint` + `app.slint`, `lab-app/src/overlay.rs` (new) |
| 4 | POLA-55 | M1 findings 3–4 | Engineer Sonnet | `zoom_toward` in `camera.rs`, `project_detail` in `lab-app/src/views.rs` |

Tier and stage changes: POLA-49 drops to Opus Low (the orthographic camera and crisp holes exist; what is left is a material swap). POLA-52 and POLA-53 move to stage 2: they share no file with POLA-49/50 except additive lines in `lib.rs`, and stage 3 is then POLA-51 alone, because split view changes how every system finds "the" camera and must land on a settled `camera.rs`. POLA-54 stays after split so the overlay is placed in the Top pane.

**Merges (Engineer Sonnet).** Stage 2 branches all add variants to `lab-viewport/src/lib.rs` (`ViewportRequest`, `FrameRequest`, `ViewportEvent`, `Tool`), plugins to the viewport app, and buttons to the `app.slint` toolbar; keep both sides (`Integration fix:` commit if needed).

## Shared interfaces (`crates/lab-viewport/src/lib.rs`)

Each task adds only its own lines, exactly as written. A new `ViewportRequest` variant gets the matching `FrameRequest` variant and is forwarded like the existing ones.

```rust
pub enum ViewportRequest {
    /* … existing … */
    /// POLA-49. Flat "blueprint" look; applies only while a camera is in Top mode.
    SetBlueprint(bool),
    /// POLA-52. Show only this platform, the platforms stacked on it and their nodes; hide room,
    /// walls, fixtures and everything else. `None` releases. Unknown ids release.
    Isolate(Option<PlatformId>),
    /// POLA-52. Windows "reduce animations": camera moves jump instead of animating.
    SetReduceMotion(bool),
}

pub enum Tool { #[default] Select, Move, Rotate, /** POLA-53 */ Measure }

pub enum ViewMode {
    #[default] Perspective,
    Top,
    /// POLA-51. Left half Perspective, right half Top, in one texture.
    Split,
}

pub enum ViewportEvent {
    /* … existing … */
    /// POLA-52. The isolated platform after an `Isolate` request (None = released, also when
    /// the platform was deleted).
    IsolationChanged(Option<PlatformId>),
    /// POLA-53. Current measurement in the room frame (mm, +Z up); `b` is None while the second
    /// point is not picked yet. `None` = cleared.
    Measured(Option<Measurement>),
    /// POLA-54. Overlay items for the Top camera, sent after each frame in which the Top camera or
    /// the document changed. Empty when no camera is in Top mode.
    Overlay(Vec<OverlayItem>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement { pub a: bevy::math::DVec3, pub b: Option<bevy::math::DVec3> }

#[derive(Clone, Debug, PartialEq)]
pub enum OverlayItem {
    /// Name label anchored at the target's projected origin (node) or top-left corner (platform).
    Label { target: SelectionTarget, pos: Vec2 },
    /// Dimension line between two projected points; the app formats `length_mm` in the display unit.
    Dimension { a: Vec2, b: Vec2, length_mm: f64 },
}
```

All screen positions are **physical px relative to the whole viewport** (the same space as `ViewportInput`), so the app never needs to know the split layout.

---

## Stage 2

### POLA-49 Top view: blueprint look and plate framing (Engineer Opus Low)
- `SetBlueprint(bool)`: in Top mode (and, after POLA-51, the Top half of Split) part and platform materials swap to flat unlit colours: platform fill dark blue-grey, parts light grey, hole circles drawn as outlines (add a `blueprint: u32` field to the hole material params and a branch in `hole_grid.wesl`). Swap back when blueprint is off or the camera leaves Top. Keep the original materials (as `picking.rs` `HighlightLook` does); do not rebuild meshes. Highlights (hover/selection tint) still apply on top.
- Toolbar toggle "Blueprint" next to the 3D/Top control, visible only in Top; the value is a session setting (not saved in the document).
- In Top mode with a platform selected, **F** frames that platform (`FrameTarget::Platform`); with nothing selected, Shift+F/F frame all as today.

**Works when:** Top view with Blueprint on shows the table flat with outlined holes and grey parts, off restores the shaded look, and zooming to 0.05 m keeps holes crisp.

### POLA-50 Selection outline and part hover (Engineer Opus Low)
- Outline by **inverted hull**, not a post-process: for the selected node (and each descendant mesh of it, so a whole stack outlines), spawn a child entity per mesh with the same mesh handle, `Transform::from_scale(1.0 + s)` around the mesh centre with `s` chosen for ~2 px at the current distance (recompute when the camera settles, not every frame), an unlit accent-coloured material (`#0A84FF`) with `cull_mode: Some(Face::Front)`, `NotShadowCaster`, no `PickId`. Despawn on deselect. Platforms keep the existing tint.
- Hover: confirm `Shape::Part` pieces get the existing hover tint (they carry `PickId`); if not, add it in `picking.rs`. Hover of a mated child tints only that node.
- F / Shift+F already work: check once, do not rewrite.

**Works when:** clicking a placed KM100 stack shows a blue outline around every piece of the selected node, hovering another part tints it, and clicking empty space removes the outline.

### POLA-52 Timed view transitions and isolate plate (Engineer Opus Low; moved from stage 3)
- Transitions: replace the exponential ease in `animate_camera` for **mode changes and Frame requests** with a timed 350 ms ease-out (cubic) from the current view to the target (§7 Motion); drags stay 1:1 and wheel zoom keeps the exponential ease. If Windows "reduce animations" is on (`SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)`, read once at start in `lab-app` and sent as `ViewportRequest::SetReduceMotion`), jump directly. Perspective↔Top must not flip or roll: interpolate yaw the short way and switch to orthographic only at the end (keep the existing `orthographic` threshold).
- Isolate (`isolate.rs`): on `Isolate(Some(p))` set `Visibility::Hidden` on every scene entity that is not platform `p`, a platform whose `OnPlatform` chain reaches `p`, or a node whose `root_platform` is one of those; then `Frame(Platform(p))`. Re-apply after `LoadDocument`/`ApplyChanges` (new entities). Hidden entities are not pickable. `Isolate(None)` restores visibility and frames all. Send `IsolationChanged`.
- UI: "Isolate" in the platform inspector and as a scene-row action `"isolate"` on platform rows; while isolated a small banner at the top of the viewport ("Isolated: Table A · Show all") whose button and **Esc** (when no tool or rename is active) release it. Isolation is session state, not saved.

**Works when:** 3D→Top→3D animates in about 350 ms without a flip, and isolating Table A in a document with two tables and a room hides the other table and the walls until "Show all".

### POLA-53 Measure tool (Engineer Opus Low; moved from stage 3)
- `Tool::Measure` (toolbar button "Measure", key **M**). First click sets `a`, second sets `b`, a third click starts a new measurement; **Esc** or switching tool clears it (`Measured(None)`).
- Snapping (screen space, 12 px): part origins of placed nodes (from `Document::node_pose`), then platform holes (`lab_mates::snap` on the platform under the ray), else the ray hit on the scene, else the floor plane. Points are in the room frame (mm).
- Drawing: Bevy `Gizmos` line from `a` to `b` (or to the snapped point under the pointer while `b` is open), plus small spheres at the ends; request frames only on pointer move while the tool is active, so the app still idles.
- App (`lab-app/src/measure.rs`): readout chip at the bottom of the viewport: `123.4 mm  (Δx 100.0, Δy 70.0, Δz 20.0)` in the current display unit (use the existing mm/in formatting helpers).

**Works when:** measuring between two holes 4 pitches apart on an M6 table shows 100.0 mm (3.937 in in inch display), and the app returns to 0 frames when the pointer stops.

---

## Stage 3

### POLA-51 Split view (Engineer Opus)
- **Decision: one texture, two cameras** with `Camera::viewport` set to the left and right halves (recomputed whenever the render target is resized or rescaled by the adaptive scaler). This keeps the UI side, the texture channel and the frame pacing unchanged; DESIGN §6.1's "two textures" is updated in Hardening. The intent (two independent views, idle when nothing changes) is met.
- `ViewMode::Split`: the existing camera keeps its `OrbitCamera` in Perspective on the left; a second `Camera3d` with its own `OrbitCamera` (Top) is spawned on the right (despawned or deactivated outside Split). Both share the scene, lights, MSAA and quality settings. `SetBlueprint` applies to the Top camera only.
- Input routing: a press decides the pane by its x; drags, wheel, picking, placement ghosts, gizmo and measure use the camera under the pointer (one helper, e.g. `fn camera_at(pos) -> Entity`, replacing each `Single` camera query). `Frame` requests apply to both cameras.
- UI: the segmented control becomes `["3D", "Top", "Split"]`; a 1 px divider line drawn by Slint over the seam.
- Transitions into and out of Split use POLA-52's timed animation for the Top camera.

**Works when:** Split shows 3D and Top side by side, orbiting the left pane does not move the right, dragging a part from the library into either pane places it, and the viewport returns to 0 frames when idle (check `FrameReady` stops).

---

## Stage 4

### POLA-54 Top-view labels and dimensions overlay (Engineer Opus Low)
- `overlay.rs`: after a frame in which the Top camera (Top or the right half of Split) or the document changed, project and send `Overlay`: one `Label` per platform and per node whose root is the isolated or visible platform (cap at 200 labels; skip labels closer than 40 px to an earlier one), and two `Dimension`s per platform (outline bounding-box width along the top edge, depth along the left edge, offset 24 px outward). During a camera animation send at most one `Overlay` per frame; nothing when idle.
- Slint: an overlay layer above the viewport image (`overlays.slint`) drawing labels (`Text`, 11 px, background pill) and dimension lines (`Path` with end ticks and the formatted length centred). `lab-app/src/overlay.rs` maps `OverlayItem`s to a Slint model, formatting lengths in the display unit and names from the document. A toolbar toggle "Labels" (on by default in Top).

**Works when:** in Top view a table with three placed parts shows the table name, its width and depth dimensions and three part labels, and they stay attached while panning and zooming.

### POLA-55 M1 findings 3–4 (Engineer Sonnet)
- **Zoom near a surface (§14 finding 3).** In `zoom_toward`, when zooming in, ray-cast the pointer against the scene (the picking meshes, as `pick` does) or, if nothing is hit, the floor plane; move `target.focus` onto that hit point before scaling the distance, so the distance shrinks toward the surface rather than toward the old focal plane. The existing 0.05 m clamp then becomes reachable. Zooming out is unchanged.
- **Header count (finding 4).** `project_detail` in `views.rs` counts each `PlatformKind` separately: "1 table", "2 tables, 1 breadboard, 1 custom platform"; zero kinds omitted. Update its unit test if one exists, else add one.

**Works when:** in 3D, scrolling toward a point on the table top reaches about 0.05 m from it, and a document with a table, a breadboard and a custom platform shows "1 table, 1 breadboard, 1 custom platform".

---

## M3 done

After stage 4 merges, the Lead checks the exit criterion from the reports: 3D↔Top switches with the timed transition (POLA-52) and one plate isolates (POLA-52), with split, measure, outline and overlay present. Hardening (M5) tests it end to end.
