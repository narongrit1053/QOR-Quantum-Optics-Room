//! Polarization of the beam axis.
//!
//! Conventions: time dependence e^{-iωt}; the slow axis of a waveplate gets e^{+iΓ}; angles are
//! measured from the lab horizontal, positive counter-clockwise as seen by the receiver (looking
//! toward the source, horizontal to the right, vertical up); handedness follows Hecht, so
//! positive ellipticity is right-handed.

use glam::DVec3;

/// Complex field vector `re + i·im`, transverse to the beam, unit power at the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub re: DVec3,
    pub im: DVec3,
}

impl Field {
    pub fn power(&self) -> f64 {
        self.re.length_squared() + self.im.length_squared()
    }

    pub fn scale(self, k: f64) -> Field {
        Field { re: self.re * k, im: self.im * k }
    }

    /// Applies a real-linear map to both parts.
    pub fn map(self, f: impl Fn(DVec3) -> DVec3) -> Field {
        Field { re: f(self.re), im: f(self.im) }
    }

    /// Complex `E·a` as (re, im).
    pub fn dot(&self, a: DVec3) -> (f64, f64) {
        (self.re.dot(a), self.im.dot(a))
    }
}

/// Polarization state: azimuth ψ of the major axis in [0, 180) and ellipticity angle χ in [-45, 45].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Polarization {
    pub azimuth_deg: f64,
    pub ellipticity_deg: f64,
}

impl Polarization {
    pub fn label(&self) -> String {
        let (psi, chi) = (self.azimuth_deg, self.ellipticity_deg);
        if chi.abs() > 44.5 {
            format!("Circular {}", if chi > 0.0 { "R" } else { "L" })
        } else if chi.abs() < 0.5 {
            format!("Linear {psi:.0}°")
        } else {
            format!("Elliptical {psi:.0}°, {chi:.0}°")
        }
    }
}

/// Lab horizontal for a beam along `dir`: `Z × dir`. A vertical beam has none, so `fallback_x`
/// projected onto the plane ⊥ `dir` stands in. The vertical is then `dir × h`.
pub fn horizontal(dir: DVec3, fallback_x: DVec3) -> DVec3 {
    let h = DVec3::Z.cross(dir);
    if h.length() >= 1e-9 {
        return h.normalize();
    }
    (fallback_x - dir * fallback_x.dot(dir)).normalize_or_zero()
}

/// Unit-power field of state `p` for a beam along `dir` with horizontal `h`.
pub fn field_from(p: Polarization, dir: DVec3, h: DVec3) -> Field {
    let v = dir.cross(h);
    let (psi, chi) = (p.azimuth_deg.to_radians(), p.ellipticity_deg.to_radians());
    let (ex, ey) = ((psi.cos() * chi.cos(), psi.sin() * chi.sin()), (psi.sin() * chi.cos(), -psi.cos() * chi.sin()));
    Field { re: h * ex.0 + v * ey.0, im: h * ex.1 + v * ey.1 }
}

/// Polarization state of `f` for a beam along `dir` with horizontal `h`.
pub fn polarization_of(f: &Field, dir: DVec3, h: DVec3) -> Polarization {
    let v = dir.cross(h);
    let ((xr, xi), (yr, yi)) = (f.dot(h), f.dot(v));
    let s0 = xr * xr + xi * xi + yr * yr + yi * yi;
    let s1 = xr * xr + xi * xi - yr * yr - yi * yi;
    let s2 = 2.0 * (xr * yr + xi * yi);
    let s3 = 2.0 * (xi * yr - xr * yi);
    let mut psi = (0.5 * s2.atan2(s1)).to_degrees().rem_euclid(180.0);
    if psi > 180.0 - 1e-9 {
        psi = 0.0; // rounding of a tiny negative angle
    }
    let chi = if s0 > 0.0 { 0.5 * (s3 / s0).clamp(-1.0, 1.0).asin() } else { 0.0 };
    Polarization { azimuth_deg: psi, ellipticity_deg: chi.to_degrees() }
}

/// Reflection off a mirror with unit normal `n`.
pub fn mirror(f: Field, n: DVec3) -> Field {
    f.map(|e| 2.0 * e.dot(n) * n - e)
}

/// Fast axis of a plate with normal `n` at `angle_deg` from the lab horizontal, projected ⊥ `d`.
pub fn plate_axis(n: DVec3, frame_x: DVec3, angle_deg: f64, d: DVec3) -> DVec3 {
    let hn = horizontal(n, frame_x);
    let (s, c) = angle_deg.to_radians().sin_cos();
    let f0 = c * hn + s * n.cross(hn);
    (f0 - f0.dot(d) * d).normalize_or_zero()
}

/// Waveplate with unit fast axis `fast` ⊥ `d`: the slow axis `d × fast` gets phase e^{iΓ}.
pub fn waveplate(f: Field, d: DVec3, fast: DVec3, retardance: f64) -> Field {
    let s = d.cross(fast);
    let ((fr, fi), (sr, si)) = (f.dot(fast), f.dot(s));
    let (sin, cos) = retardance.sin_cos();
    let (pr, pi) = (sr * cos - si * sin, sr * sin + si * cos);
    Field { re: fast * fr + s * pr, im: fast * fi + s * pi }
}

/// Polarizing beam splitter with plane normal `n`: (transmitted, reflected before the mirror flip).
/// The s component (⊥ the plane of incidence) reflects.
pub fn pbs(f: Field, d: DVec3, n: DVec3) -> (Field, Field) {
    let s = d.cross(n);
    if s.length() < 1e-9 {
        return (f, Field { re: DVec3::ZERO, im: DVec3::ZERO });
    }
    let s = s.normalize();
    let r = Field { re: s * f.re.dot(s), im: s * f.im.dot(s) };
    (Field { re: f.re - r.re, im: f.im - r.im }, r)
}
