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

mod camera;
mod gizmo;
mod hole_material;
mod host;
mod measure;
mod mesh;
mod picking;
mod place;
mod platform;
mod room;
mod scene;
mod sync;

use std::sync::Arc;

use lab_core::{ChangeSet, Document, PlatformId};
pub use lab_core::{SelectionTarget, WallDisplay};

pub use bevy::math::Vec2;
pub use host::{ViewportHandle, start};
pub use place::stack_chain;
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Move,
    Rotate,
    /// POLA-53
    Measure,
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
        attach: lab_core::Attach,
        position: bevy::math::DVec3,
        rotation: bevy::math::DQuat,
        done: bool,
    },
    /// POLA-53. Current measurement in the room frame (mm, +Z up); `b` is None while the second
    /// point is not picked yet. `None` = cleared.
    Measured(Option<Measurement>),
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
}

/// The catalog from [`ViewportRequest::SetCatalog`] (empty until the app sends one).
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct CatalogRes(pub Arc<lab_catalog::Catalog>);
