//! Per-user display preferences. They live in the user's settings, never in the document, so
//! changing them never marks a document edited and never adds an undo step (M1-SPEC D10).

use serde::{Deserialize, Serialize};

/// Colour scheme of the UI chrome (the 3D viewport keeps its look in every mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    HighContrast,
}

/// When room walls are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WallDisplay {
    /// Walls fade out when they block the view.
    #[default]
    Auto,
    Always,
    Hidden,
}
