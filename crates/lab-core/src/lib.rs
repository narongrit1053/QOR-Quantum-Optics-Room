//! The LabSetup document model.
//!
//! The [`Document`] is the single source of truth for a project. Every edit goes through a
//! [`Command`], which returns its own inverse (for undo) and a [`ChangeSet`] describing what
//! changed. Renderers and UI view-models are projections that update from change sets; they
//! never mutate the document directly.
//!
//! Conventions: lengths are millimetres ([`Mm`]), angles are radians, +Z is up. A node's
//! placement is relative to its platform frame, whose origin is platform-local `(0, 0)` on the
//! top surface. Presets and parametric outlines centre the outline's bounding box on that origin.
//! Custom polygons are typed in that frame, and editing an outline never moves the frame.

pub mod command;
pub mod history;
pub mod holes;
pub mod limits;
pub mod migrate;
pub mod model;
pub mod outline;
pub mod part;
pub mod prefs;
pub mod presets;
pub mod room;
pub mod sample;
pub mod selection;
pub mod units;
pub mod validate;

pub use command::{ChangeSet, Command, CommandError, PlatformAspects};
pub use glam;
pub use history::History;
pub use holes::*;
pub use migrate::{LoadError, LoadReport};
pub use model::*;
pub use outline::*;
pub use prefs::{ThemeMode, WallDisplay};
pub use room::*;
pub use selection::SelectionTarget;
pub use units::{LengthUnit, Mm};
pub use validate::{GridError, PlatformError, RoomError};
