//! The LabSetup document model.
//!
//! The [`Document`] is the single source of truth for a project. Every edit goes through a
//! [`Command`], which returns its own inverse (for undo) and a [`ChangeSet`] describing what
//! changed. Renderers and UI view-models are projections that update from change sets; they
//! never mutate the document directly.
//!
//! Conventions: lengths are millimetres ([`Mm`]), angles are radians, +Z is up. A node's
//! placement is relative to its platform: the origin is the centre of the platform's top
//! surface.

pub mod command;
pub mod model;
pub mod sample;
pub mod units;

pub use command::{ChangeSet, Command, CommandError, History};
pub use glam;
pub use model::*;
pub use units::Mm;
