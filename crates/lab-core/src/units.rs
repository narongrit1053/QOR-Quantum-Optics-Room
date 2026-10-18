//! Length units. Everything in the document is stored in millimetres.

/// Millimetres, the internal length unit.
pub type Mm = f64;

/// One inch in millimetres.
pub const INCH: Mm = 25.4;

/// Converts inches to millimetres.
pub fn inches(value: f64) -> Mm {
    value * INCH
}

/// Formats a length for display, e.g. `"75.0 mm"`.
pub fn format_mm(value: Mm) -> String {
    format!("{value:.1} mm")
}
