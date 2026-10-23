//! The LabSetup 3D viewport.
//!
//! A Bevy app runs on its own thread and renders into wgpu textures created on the UI's own
//! device, so the UI can composite them without copies (adapted from Slint's
//! `examples/bevy/slint-hosts-bevy`). Communication is by channels only:
//!
//! - UI → viewport: [`ViewportRequest`] (input, view changes, documents, texture recycling)
//! - viewport → UI: rendered textures and [`ViewportEvent`]s
//!
//! Frames are rendered **on demand**: the Bevy app is only updated when a request arrives or a
//! system asks for another frame (e.g. while a camera animation is running). An idle viewport
//! costs no GPU time.
//!
//! The viewport never edits the document. It renders a snapshot and reports what the user
//! picked; edits go through `lab_core::Command` on the UI side.

mod beams;
mod blueprint;
mod export;
mod camera;
mod feature_pick;
mod gizmo;
mod glb;
mod hole_material;
mod host;
mod inspect;
mod isolate;
mod measure;
mod mesh;
mod outline;
mod overlay;
mod part_box;
mod picking;
mod place;
mod platform;
mod room;
mod scene;
mod sync;
mod thumbnail;

use std::sync::Arc;

use lab_core::{ChangeSet, Document, PlatformId};
pub use lab_core::{SelectionTarget, WallDisplay};

pub use bevy::math::Vec2;
pub use host::{ViewportHandle, smoke_test, start};
pub use place::stack_chain;
pub use thumbnail::read_thumbnail;
pub use wgpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Pointer input in **physical pixels**, relative to the viewport's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewportInput {
    PointerDown { button: PointerButton, pos: Vec2, mods: Modifiers },
    PointerUp { button: PointerButton, pos: Vec2, mods: Modifiers },
    PointerMove { pos: Vec2, mods: Modifiers },
    PointerLeave,
    /// `delta.y > 0` means scrolling up / away from the user (zoom in).
    Scroll { delta: Vec2, pos: Vec2, mods: Modifiers },
}

/// Rendering quality. Lower settings trade looks for GPU time on integrated graphics.
/// Pass `None` to [`start`] to pick one from the GPU type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bevy::prelude::Resource)]
pub enum Quality {
    /// 4× MSAA and directional shadows.
    #[default]
    High,
    /// 4× MSAA, no shadows. Default on integrated GPUs.
    Balanced,
    /// No MSAA, no shadows: for very weak GPUs and battery use.
    Low,
}

impl Quality {
    pub fn msaa_samples(self) -> u32 {
        match self {
            Quality::High | Quality::Balanced => 4,
            Quality::Low => 1,
        }
    }

    pub fn shadows(self) -> bool {
        self == Quality::High
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    /// Free 3D orbit view with perspective projection.
    #[default]
    Perspective,
    /// Orthographic view straight down onto the tables.
    Top,
    /// POLA-51. Left half Perspective, right half Top, in one texture.
    Split,
}

/// What [`ViewportRequest::Frame`] fits into view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameTarget {
    /// Every platform (and the parts on them); the room when there are no platforms.
    All,
    /// The current selection. Nothing happens when nothing is selected.
    Selection,
    Platform(PlatformId),
    Room,
}

pub enum ViewportRequest {
    Input(ViewportInput),
    SetViewMode(ViewMode),
    /// Animate the camera so the target fits. Unknown ids are ignored.
    Frame(FrameTarget),
    /// Select something from outside the viewport (e.g. the scene list).
    Select(Option<SelectionTarget>),
    /// Replace the rendered scene with this document snapshot (full rebuild). Clears the
    /// selection and hover.
    LoadDocument(Box<Document>),
    /// Incremental update: `snapshot` is the document after `changes`.
    ApplyChanges { snapshot: Box<Document>, changes: ChangeSet },
    SetWallDisplay(WallDisplay),
    /// Size of the viewport in physical pixels.
    Resize { width: u32, height: u32 },
    /// Hand a texture back once the UI no longer displays it.
    ReleaseTexture(wgpu::Texture),
    /// The part catalog used to build part meshes and ghosts.
    SetCatalog(Arc<lab_catalog::Catalog>),
    /// A library drag over the viewport.
    DragPart(PartDrag),
    /// The pointer tool: selecting, or the move/rotate gizmo on the selected node.
    SetTool(Tool),
    /// POLA-49. Flat "blueprint" look; applies only while a camera is in Top mode.
    SetBlueprint(bool),
    /// POLA-52. Show only this platform, the platforms stacked on it and their nodes; hide room,
    /// walls, fixtures and everything else. `None` releases. Unknown ids release.
    Isolate(Option<PlatformId>),
    /// POLA-52. Windows "reduce animations": camera moves jump instead of animating.
    SetReduceMotion(bool),
    /// POLA-60. Show only catalog part `id` at the origin, unrotated, framed; the document is
    /// hidden but kept. `None` returns to the document and restores the camera.
    InspectPart(Option<String>),
    /// POLA-60. Connector/port frames (part space, mm) drawn as small axis arrows while inspecting.
    InspectMarkers(Vec<lab_core::part::Frame>),
    /// POLA-61. Render catalog part `part` offscreen (256×256, transparent background, 3/4 view)
    /// and write a PNG to `path`. Queued; one at a time.
    RenderThumbnail { part: String, path: std::path::PathBuf },
    /// POLA-66. Render the main camera (its transform and projection, the document as shown)
    /// offscreen at `width`×`height` (clamped to 16..=8192), opaque background, MSAA 4, and
    /// write a PNG to `path`. In Split mode the 3D pane is exported.
    ExportImage { path: std::path::PathBuf, width: u32, height: u32 },
    /// M6. Show or hide the traced laser beams (default shown).
    SetBeams(bool),
    /// M8 view cube. Ease the 3D camera to this direction (as `CameraOrient` reports it) around
    /// the orbit pivot, keeping the distance. Ignored outside the 3D view.
    ViewDirection { yaw: f32, pitch: f32 },
    /// M8 constrain tool. Outline this feature (in the target's frame: part frame for a node,
    /// platform frame for a platform) while the tool is active. `None` clears.
    HighlightFeature(Option<(SelectionTarget, lab_core::part::Feature)>),
    /// M9 D. These nodes and their descendants are not drawn or picked (replaces the last set).
    SetHidden(Vec<lab_core::NodeId>),
    /// M9 D. Show only these nodes (+descendants, + their root platform); hide room, walls and
    /// fixtures like `Isolate`. `None` releases.
    IsolateNodes(Option<Vec<lab_core::NodeId>>),
    /// M9 G. Hover outline on these nodes (empty clears).
    HighlightNodes(Vec<lab_core::NodeId>),
    /// M9 F. Open the floating part box on this node (`None` closes it). Its frames arrive on
    /// [`ViewportHandle::part_box_textures`].
    PartBox(Option<PartBoxSpec>),
    /// M9 F. Input in the box's own physical px.
    PartBoxInput(ViewportInput),
    /// M9 F. Hand a part box texture back once the UI no longer displays it.
    ReleasePartBoxTexture(wgpu::Texture),
}

/// M9 F. The node shown in the part box and the box texture size in physical px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartBoxSpec {
    pub node: lab_core::NodeId,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    /// POLA-53
    Measure,
    /// M8. Clicks report [`ViewportEvent::FeaturePoint`] instead of selecting.
    Constrain,
    /// M9. Joint tool (docs/M9-PLAN.md E gives it behaviour).
    Joint,
}

/// What a library drag carries.
#[derive(Clone, Debug, PartialEq)]
pub enum DragItem {
    /// Catalog id.
    Part(String),
    /// A solved post stack (M2-PLAN POLA-46).
    Stack(StackSpec),
}

impl DragItem {
    /// The catalog id of the part that stands on the platform.
    pub fn root(&self) -> &str {
        match self {
            DragItem::Part(id) => id,
            DragItem::Stack(s) => s.base.as_deref().unwrap_or(&s.holder),
        }
    }
}

/// Catalog ids of a beam-height stack, bottom to top, and the post's slide in the holder.
#[derive(Clone, Debug, PartialEq)]
pub struct StackSpec {
    pub optic: Option<String>,
    pub mount: String,
    pub base: Option<String>,
    pub holder: String,
    pub post: String,
    pub slide: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PartDrag {
    /// Pointer over the viewport while dragging (physical px, viewport-relative).
    Move { item: DragItem, pos: Vec2, mods: Modifiers },
    Drop { pos: Vec2, mods: Modifiers },
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewportEvent {
    Ready { adapter: String, backend: String, quality: Quality, msaa_samples: u32 },
    /// A new texture is waiting on the texture channel. `frames_rendered` counts every frame
    /// since start, which makes on-demand rendering easy to verify.
    FrameReady { frames_rendered: u64 },
    SelectionChanged(Option<SelectionTarget>),
    /// Nodes only in M1.
    HoverChanged(Option<SelectionTarget>),
    /// An item could not be built and was skipped. The text is a complete sentence.
    SceneError(String),
    /// A library drag was dropped where the item can go.
    PlaceRequested(PlaceRequest),
    /// A gizmo drag moved node `id` (relative to its parent, as `Node` stores it). `done` ends the
    /// gesture: the app closes the undo step.
    TransformRequested {
        id: lab_core::NodeId,
        /// Usually the node's own parent; its root (platform or room) when a move unmates it.
        parent: lab_core::Parent,
        attach: lab_core::Attach,
        position: bevy::math::DVec3,
        rotation: bevy::math::DQuat,
        done: bool,
    },
    /// POLA-52. The isolated platform after an `Isolate` request (None = released, also when
    /// the platform was deleted).
    IsolationChanged(Option<PlatformId>),
    /// POLA-53. Current measurement in the room frame (mm, +Z up); `b` is None while the second
    /// point is not picked yet. `None` = cleared.
    Measured(Option<Measurement>),
    /// POLA-54. Overlay items for the Top camera, sent after each frame in which the Top camera or
    /// the document changed. Empty when no camera is in Top mode.
    Overlay(Vec<OverlayItem>),
    /// POLA-60. A click on the inspected part: hit point and surface normal, part space, mm.
    PartPoint { point: bevy::math::DVec3, normal: bevy::math::DVec3 },
    /// POLA-61. The PNG for `part` is written (`ok` false on failure; the app skips it then).
    ThumbnailReady { part: String, path: std::path::PathBuf, ok: bool },
    /// POLA-66. Sent once per `ExportImage`; `ok` false on any failure.
    ImageExported { path: std::path::PathBuf, ok: bool },
    /// M8. The main camera's orientation (render space: eye = focus + d·(cos p sin y, sin p,
    /// cos p cos y), Y up), sent when it turns by > 0.002 rad.
    CameraOrient { yaw: f32, pitch: f32 },
    /// M8. A click with the constrain tool: hit point and surface normal in the target's frame
    /// (part frame for a node, platform frame for a platform), mm, +Z up.
    FeaturePoint { target: SelectionTarget, point: bevy::math::DVec3, normal: bevy::math::DVec3 },
    /// M9 D. A right click without drag (the target is selected first); `pos` in physical px.
    ContextMenu { target: Option<SelectionTarget>, pos: Vec2 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum OverlayItem {
    /// Name label anchored at the target's projected origin (node) or top-left corner (platform).
    Label { target: SelectionTarget, pos: Vec2 },
    /// Dimension line between two projected points; the app formats `length_mm` in the display unit.
    Dimension { a: Vec2, b: Vec2, length_mm: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement { pub a: bevy::math::DVec3, pub b: Option<bevy::math::DVec3> }

/// Where a dropped item goes. `position`/`rotation` are relative to `parent`, as `Node` stores them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceRequest {
    pub item: DragItem,
    pub parent: lab_core::Parent,
    pub attach: lab_core::Attach,
    pub position: bevy::math::DVec3,
    pub rotation: bevy::math::DQuat,
}

/// Requests that reach the Bevy world for the next update (everything except texture
/// management, which the host loop handles itself).
#[derive(Clone, Debug)]
pub(crate) enum FrameRequest {
    Input(ViewportInput),
    SetViewMode(ViewMode),
    Frame(FrameTarget),
    Select(Option<SelectionTarget>),
    LoadDocument(Box<Document>),
    ApplyChanges(Box<Document>, ChangeSet),
    #[allow(dead_code)] // Read by the wall fade (room.rs, T4c).
    SetWallDisplay(WallDisplay),
    DragPart(PartDrag),
    SetTool(Tool),
    SetBlueprint(bool),
    Isolate(Option<PlatformId>),
    SetReduceMotion(bool),
    InspectPart(Option<String>),
    InspectMarkers(Vec<lab_core::part::Frame>),
    RenderThumbnail(String, std::path::PathBuf),
    ExportImage(std::path::PathBuf, u32, u32),
    SetBeams(bool),
    ViewDirection(f32, f32),
    HighlightFeature(Option<(SelectionTarget, lab_core::part::Feature)>),
    SetHidden(Vec<lab_core::NodeId>),
    IsolateNodes(Option<Vec<lab_core::NodeId>>),
    HighlightNodes(Vec<lab_core::NodeId>),
    PartBox(Option<PartBoxSpec>),
    PartBoxInput(ViewportInput),
}

/// The catalog from [`ViewportRequest::SetCatalog`] (empty until the app sends one).
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct CatalogRes(pub Arc<lab_catalog::Catalog>);
