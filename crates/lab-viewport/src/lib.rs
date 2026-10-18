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
mod host;
mod picking;
mod scene;

use lab_core::{Document, NodeId};

pub use bevy::math::Vec2;
pub use host::{ViewportHandle, start};
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

pub enum ViewportRequest {
    Input(ViewportInput),
    SetViewMode(ViewMode),
    /// Animate the camera so the whole scene fits.
    FrameAll,
    /// Select a node from outside the viewport (e.g. the scene list).
    Select(Option<NodeId>),
    /// Replace the rendered scene with this document snapshot.
    LoadDocument(Box<Document>),
    /// Size of the viewport in physical pixels.
    Resize { width: u32, height: u32 },
    /// Hand a texture back once the UI no longer displays it.
    ReleaseTexture(wgpu::Texture),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewportEvent {
    Ready { adapter: String, backend: String, quality: Quality, msaa_samples: u32 },
    /// A new texture is waiting on the texture channel. `frames_rendered` counts every frame
    /// since start, which makes on-demand rendering easy to verify.
    FrameReady { frames_rendered: u64 },
    SelectionChanged(Option<NodeId>),
    HoverChanged(Option<NodeId>),
}

/// Requests that reach the Bevy world for the next update (everything except texture
/// management, which the host loop handles itself).
#[derive(Clone, Debug)]
pub(crate) enum FrameRequest {
    Input(ViewportInput),
    SetViewMode(ViewMode),
    FrameAll,
    Select(Option<NodeId>),
    LoadDocument(Box<Document>),
}
