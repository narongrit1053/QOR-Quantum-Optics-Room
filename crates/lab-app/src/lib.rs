//! The LabSetup application logic library.
//!
//! Holds the pure, testable application layer: session management, edit pipeline
//! and UI view-models. Free of Slint runtime types, windowing and GPU dependencies.

pub mod bom_rows;
pub mod edit;
pub mod session;
pub mod views;

pub use edit::*;
pub use session::*;
pub use views::*;
