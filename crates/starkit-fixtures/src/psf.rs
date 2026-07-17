//! PSF profiles: circular Moffat and elliptical Gaussian (FIXTURES.md
//! "Rendering model").
//!
//! Profiles here are *unnormalised* shape functions. Absolute flux is set by the
//! renderer, which normalises the integrated stamp to the star's catalog flux —
//! that way the truth catalog is exact by construction rather than exact up to
//! the profile's truncation error.

use serde::{Deserialize, Serialize};

/// FWHM = 2·σ·√(2·ln 2) for a Gaussian.
const FWHM_PER_SIGMA: f64 = 2.354_820_045_030_949_4;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PsfKind {
    /// Circular Moffat with shape parameter β.
    Moffat { beta: f64 },
    /// Elliptical Gaussian.
    Gaussian,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Psf {
    pub kind: PsfKind,
    /// Geometric mean of the two axes' FWHM, pixels.
    pub fwhm: f64,
    /// `1 - b/a`. Always 0 for [`PsfKind::Moffat`].
    pub ellipticity: f64,
    /// Major-axis angle, radians CCW from +x.
    pub theta: f64,
}

impl Psf {
    /// Major / minor axis FWHM, chosen so their geometric mean is `self.fwhm`.
    fn axes(&self) -> (f64, f64) {
        // e = 1 - b/a and √(a·b) = fwhm  ⇒  a = fwhm/√(1-e), b = fwhm·√(1-e).
        let s = (1.0 - self.ellipticity).sqrt();
        (self.fwhm / s, self.fwhm * s)
    }

    /// Unnormalised profile value at an offset from the star centre, in pixels.
    pub fn value(&self, dx: f64, dy: f64) -> f64 {
        let (fwhm_a, fwhm_b) = self.axes();
        let (sin, cos) = self.theta.sin_cos();
        // Rotate into the PSF frame: u along the major axis, v along the minor.
        let u = dx * cos + dy * sin;
        let v = -dx * sin + dy * cos;

        match self.kind {
            PsfKind::Gaussian => {
                let sa = fwhm_a / FWHM_PER_SIGMA;
                let sb = fwhm_b / FWHM_PER_SIGMA;
                (-0.5 * ((u / sa).powi(2) + (v / sb).powi(2))).exp()
            }
            PsfKind::Moffat { beta } => {
                // α from FWHM: FWHM = 2·α·√(2^(1/β) − 1).
                let alpha_a = fwhm_a / (2.0 * (2.0_f64.powf(1.0 / beta) - 1.0).sqrt());
                let alpha_b = fwhm_b / (2.0 * (2.0_f64.powf(1.0 / beta) - 1.0).sqrt());
                let r2 = (u / alpha_a).powi(2) + (v / alpha_b).powi(2);
                (1.0 + r2).powf(-beta)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance: the half-maximum crossing is checked analytically, so the only
    /// error is f64 round-off in a handful of transcendental calls.
    const EPS: f64 = 1e-12;

    fn at_half_max(psf: &Psf, along_major: bool) -> f64 {
        let (a, b) = psf.axes();
        let half = if along_major { a / 2.0 } else { b / 2.0 };
        let (dx, dy) = if along_major {
            (half * psf.theta.cos(), half * psf.theta.sin())
        } else {
            (-half * psf.theta.sin(), half * psf.theta.cos())
        };
        psf.value(dx, dy) / psf.value(0.0, 0.0)
    }

    #[test]
    fn gaussian_reaches_half_max_at_half_fwhm_on_both_axes() {
        let psf = Psf {
            kind: PsfKind::Gaussian,
            fwhm: 3.2,
            ellipticity: 0.12,
            theta: 0.7,
        };
        assert!((at_half_max(&psf, true) - 0.5).abs() < EPS);
        assert!((at_half_max(&psf, false) - 0.5).abs() < EPS);
    }

    #[test]
    fn moffat_reaches_half_max_at_half_fwhm_for_every_beta() {
        for beta in [2.5, 3.0, 3.7, 4.5] {
            let psf = Psf {
                kind: PsfKind::Moffat { beta },
                fwhm: 3.2,
                ellipticity: 0.0,
                theta: 0.0,
            };
            assert!(
                (at_half_max(&psf, true) - 0.5).abs() < EPS,
                "beta {beta} misses half-max"
            );
        }
    }

    #[test]
    fn ellipticity_stretches_the_major_axis_only() {
        let psf = Psf {
            kind: PsfKind::Gaussian,
            fwhm: 4.0,
            ellipticity: 0.15,
            theta: 0.0,
        };
        let (a, b) = psf.axes();
        assert!(a > b, "major axis must exceed minor");
        assert!(
            ((a * b).sqrt() - 4.0).abs() < EPS,
            "geometric mean of axes must be the catalog FWHM"
        );
        assert!(
            (1.0 - b / a - 0.15).abs() < EPS,
            "ellipticity must round-trip"
        );
    }
}
