//! Length units, parsing and formatting. Everything in the document is stored in millimetres and
//! radians; display units are a per-user preference (M1-SPEC D10).

use std::f64::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

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

/// The unit lengths are shown and typed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    #[default]
    Millimetre,
    Inch,
}

impl LengthUnit {
    pub fn suffix(self) -> &'static str {
        match self {
            LengthUnit::Millimetre => "mm",
            LengthUnit::Inch => "in",
        }
    }
}

/// `value` with at most `decimals` decimals, trailing zeros trimmed, and `-0` shown as `0`.
fn format_decimal(value: f64, decimals: usize) -> String {
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        let trimmed = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(trimmed);
    }
    if text == "-0" {
        text = "0".into();
    }
    text
}

/// The number only: millimetres with at most 2 decimals, inches with at most 3.
pub fn format_length(value: Mm, unit: LengthUnit) -> String {
    match unit {
        LengthUnit::Millimetre => format_decimal(value, 2),
        LengthUnit::Inch => format_decimal(value / INCH, 3),
    }
}

/// The number and its unit, e.g. `"1200 mm"` or `"47.244 in"`.
pub fn format_length_with_unit(value: Mm, unit: LengthUnit) -> String {
    format!("{} {}", format_length(value, unit), unit.suffix())
}

/// A count with thousands separators, e.g. `"1,728"`.
pub fn format_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("Enter a number")]
    Empty,
    #[error("Use a dot as the decimal separator")]
    Comma,
    #[error("“{0}” is not a number")]
    NotANumber(String),
    #[error("Unknown unit “{0}” (use mm, cm, m, in, ft)")]
    UnknownUnit(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Mm,
    Cm,
    M,
    In,
    Ft,
}

impl Unit {
    fn parse(token: &str) -> Option<Unit> {
        match token.to_ascii_lowercase().as_str() {
            "mm" => Some(Unit::Mm),
            "cm" => Some(Unit::Cm),
            "m" => Some(Unit::M),
            "in" | "inch" | "inches" | "\"" => Some(Unit::In),
            "ft" | "feet" | "'" => Some(Unit::Ft),
            _ => None,
        }
    }

    fn millimetres(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::M => 1000.0,
            Unit::In => INCH,
            Unit::Ft => 12.0 * INCH,
        }
    }
}

/// Splits an optional sign off the front: `(negative, rest)`.
fn split_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') { (true, rest) } else { (false, s.strip_prefix('+').unwrap_or(s)) }
}

/// Splits an unsigned decimal (digits with at most one '.', at least one digit) off the front.
fn split_number(s: &str) -> Option<(f64, &str)> {
    let mut end = 0;
    let mut digits = 0;
    let mut dot = false;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if c == '.' && !dot {
            dot = true;
        } else {
            break;
        }
        end = i + c.len_utf8();
    }
    if digits == 0 {
        return None;
    }
    let value: f64 = s[..end].parse().ok()?;
    value.is_finite().then_some((value, &s[end..]))
}

/// Splits a unit token off the front: a single `"` or `'`, or a run of other characters up to
/// whitespace, a digit, a sign, a dot or a quote.
fn split_unit(s: &str) -> (&str, &str) {
    if s.starts_with('"') || s.starts_with('\'') {
        return s.split_at(1);
    }
    let end = s
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | '"' | '\''))
        .map_or(s.len(), |(i, _)| i);
    s.split_at(end)
}

/// Accepts an optional sign, digits with an optional '.', optional spaces, and an optional
/// case-insensitive unit: mm | cm | m | in | inch | inches | " | ft | feet | '. A feet value may
/// be followed by an inch value ("1' 6\"", "1 ft 6 in"). No suffix means `default_unit`. No
/// exponents, no thousands separators, no fractions. A ',' with no '.' gives `Comma`.
pub fn parse_length(text: &str, default_unit: LengthUnit) -> Result<Mm, ParseError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    if t.contains(',') && !t.contains('.') {
        return Err(ParseError::Comma);
    }
    let not_a_number = || ParseError::NotANumber(t.to_string());
    let (negative, rest) = split_sign(t);
    let (value, rest) = split_number(rest).ok_or_else(not_a_number)?;
    let (token, rest) = split_unit(rest.trim_start());
    let rest = rest.trim_start();
    let magnitude = if token.is_empty() {
        if !rest.is_empty() {
            return Err(not_a_number());
        }
        match default_unit {
            LengthUnit::Millimetre => value,
            LengthUnit::Inch => value * INCH,
        }
    } else {
        match Unit::parse(token) {
            None if rest.is_empty() => return Err(ParseError::UnknownUnit(token.to_string())),
            None => return Err(not_a_number()),
            Some(Unit::Ft) if !rest.is_empty() => {
                // Feet followed by inches.
                let (extra, tail) = split_number(rest).ok_or_else(not_a_number)?;
                let (token2, tail) = split_unit(tail.trim_start());
                if !tail.trim().is_empty() {
                    return Err(not_a_number());
                }
                match Unit::parse(token2) {
                    _ if token2.is_empty() => {}
                    Some(Unit::In) => {}
                    None => return Err(ParseError::UnknownUnit(token2.to_string())),
                    Some(_) => return Err(not_a_number()),
                }
                (value * 12.0 + extra) * INCH
            }
            Some(_) if !rest.is_empty() => return Err(not_a_number()),
            Some(unit) => value * unit.millimetres(),
        }
    };
    if !magnitude.is_finite() {
        return Err(not_a_number());
    }
    Ok(if negative { -magnitude } else { magnitude })
}

/// Parses degrees ("45", "45°", "-12.5 deg") and returns radians.
pub fn parse_angle_deg(text: &str) -> Result<f64, ParseError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    if t.contains(',') && !t.contains('.') {
        return Err(ParseError::Comma);
    }
    let not_a_number = || ParseError::NotANumber(t.to_string());
    let (negative, rest) = split_sign(t);
    let (value, rest) = split_number(rest).ok_or_else(not_a_number)?;
    match rest.trim().to_ascii_lowercase().as_str() {
        "" | "°" | "deg" | "degs" | "degree" | "degrees" => {}
        _ => return Err(not_a_number()),
    }
    let degrees = if negative { -value } else { value };
    Ok(degrees.to_radians())
}

/// Degrees normalised to (-180, 180], at most 2 decimals, trailing zeros trimmed.
pub fn format_angle_deg(radians: f64) -> String {
    let text = format_decimal(normalize_angle(radians).to_degrees(), 2);
    if text == "-180" { "180".into() } else { text }
}

/// Normalises an angle into (-π, π].
pub fn normalize_angle(radians: f64) -> f64 {
    let a = radians.rem_euclid(TAU);
    if a > PI { a - TAU } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    #[test]
    fn u1_parse_and_format() {
        use LengthUnit::{Inch, Millimetre};
        assert!(close(parse_length("47.244 in", Millimetre).unwrap(), 1199.9976));
        assert_eq!(format_length(1199.9976, Inch), "47.244");
        assert_eq!(format_length(1200.0, Inch), "47.244");
        assert_eq!(parse_length("12,5", Millimetre), Err(ParseError::Comma));
        assert!(close(parse_length("2'", Millimetre).unwrap(), 609.6));
        assert!(close(parse_length("1' 6\"", Millimetre).unwrap(), 457.2));
        assert!(close(parse_length("1 ft 6 in", Millimetre).unwrap(), 457.2));
        assert!(close(parse_length("1' 6", Millimetre).unwrap(), 457.2));
        assert_eq!(parse_length("1e3", Millimetre), Err(ParseError::NotANumber("1e3".into())));
        assert_eq!(format_length(-0.0, Millimetre), "0");
        assert_eq!(format_length(-0.001, Millimetre), "0");
        assert_eq!(format_length(-0.0001, Inch), "0");
    }

    #[test]
    fn parse_length_units_and_errors() {
        use LengthUnit::{Inch, Millimetre};
        assert_eq!(parse_length("1200", Millimetre), Ok(1200.0));
        assert!(close(parse_length("10", Inch).unwrap(), 254.0));
        assert_eq!(parse_length(" -12.5 mm ", Inch), Ok(-12.5));
        assert_eq!(parse_length("+3cm", Millimetre), Ok(30.0));
        assert_eq!(parse_length("1.5 M", Millimetre), Ok(1500.0));
        assert!(close(parse_length("2 Inches", Millimetre).unwrap(), 50.8));
        assert!(close(parse_length("2\"", Millimetre).unwrap(), 50.8));
        assert!(close(parse_length("3 feet", Millimetre).unwrap(), 914.4));
        assert!(close(parse_length("-1' 6\"", Millimetre).unwrap(), -457.2));
        assert_eq!(parse_length(".5", Millimetre), Ok(0.5));
        assert_eq!(parse_length("5.", Millimetre), Ok(5.0));
        assert_eq!(parse_length("", Millimetre), Err(ParseError::Empty));
        assert_eq!(parse_length("   ", Millimetre), Err(ParseError::Empty));
        assert_eq!(parse_length("abc", Millimetre), Err(ParseError::NotANumber("abc".into())));
        assert_eq!(parse_length("5 xyz", Millimetre), Err(ParseError::UnknownUnit("xyz".into())));
        assert_eq!(parse_length("1,200.5", Millimetre), Err(ParseError::NotANumber("1,200.5".into())));
        assert_eq!(parse_length("1.2.3", Millimetre), Err(ParseError::NotANumber("1.2.3".into())));
        assert_eq!(parse_length("1/2", Millimetre), Err(ParseError::NotANumber("1/2".into())));
        assert_eq!(parse_length("5 mm 3", Millimetre), Err(ParseError::NotANumber("5 mm 3".into())));
        assert_eq!(parse_length("1' 6 cm", Millimetre), Err(ParseError::NotANumber("1' 6 cm".into())));
        assert_eq!(parse_length("1' -6", Millimetre), Err(ParseError::NotANumber("1' -6".into())));
        assert_eq!(parse_length("-", Millimetre), Err(ParseError::NotANumber("-".into())));
        let huge = "9".repeat(400);
        assert_eq!(parse_length(&huge, Millimetre), Err(ParseError::NotANumber(huge.clone())));
    }

    #[test]
    fn format_lengths() {
        use LengthUnit::{Inch, Millimetre};
        assert_eq!(format_length(1200.0, Millimetre), "1200");
        assert_eq!(format_length(12.5, Millimetre), "12.5");
        assert_eq!(format_length(0.126, Millimetre), "0.13");
        assert_eq!(format_length(-3.10, Millimetre), "-3.1");
        assert_eq!(format_length(25.4, Inch), "1");
        assert_eq!(format_length_with_unit(1200.0, Millimetre), "1200 mm");
        assert_eq!(format_length_with_unit(1200.0, Inch), "47.244 in");
        assert_eq!(format_mm(75.0), "75.0 mm");
    }

    #[test]
    fn counts() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1728), "1,728");
        assert_eq!(format_count(250_000), "250,000");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn angles() {
        assert!(close(parse_angle_deg("45").unwrap(), PI / 4.0));
        assert!(close(parse_angle_deg("45°").unwrap(), PI / 4.0));
        assert!(close(parse_angle_deg("-12.5 deg").unwrap(), (-12.5f64).to_radians()));
        assert_eq!(parse_angle_deg(""), Err(ParseError::Empty));
        assert_eq!(parse_angle_deg("4,5"), Err(ParseError::Comma));
        assert_eq!(parse_angle_deg("45 mm"), Err(ParseError::NotANumber("45 mm".into())));
        assert_eq!(format_angle_deg(PI / 2.0), "90");
        assert_eq!(format_angle_deg(PI), "180");
        assert_eq!(format_angle_deg(-PI), "180");
        assert_eq!(format_angle_deg(3.0 * PI / 2.0), "-90");
        assert_eq!(format_angle_deg((-12.5f64).to_radians()), "-12.5");
        assert_eq!(format_angle_deg(-1e-9), "0");
        assert!(close(normalize_angle(3.0 * PI), PI));
        assert!(close(normalize_angle(-PI), PI));
        assert!(close(normalize_angle(-PI / 2.0), -PI / 2.0));
        assert!(close(normalize_angle(5.0 * TAU + 0.25), 0.25));
    }

    /// In-test xorshift64 (no external crates).
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn u1_round_trip_within_display_precision() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..10_000 {
            let r = xorshift(&mut state);
            // [-20 m, 20 m) in steps of 0.01 µm.
            let x = (r % 4_000_000_000) as f64 / 100_000.0 - 20_000.0;
            for unit in [LengthUnit::Millimetre, LengthUnit::Inch] {
                let text = format_length(x, unit);
                let back = parse_length(&text, unit).unwrap();
                let precision = match unit {
                    LengthUnit::Millimetre => 0.005,
                    LengthUnit::Inch => 0.0005 * INCH,
                };
                assert!((back - x).abs() <= precision + 1e-9, "{x} {unit:?} -> {text} -> {back}");
                // Formatting the parsed value gives the same text (stable no-op rule).
                assert_eq!(format_length(back, unit), text, "{x} {unit:?}");
            }
        }
    }
}
