//! Tiered, feathered star masks (task T1-4, FR-4).
//!
//! The mask is the product's first user-visible win: FR-4 calls it "a deliverable
//! in its own right", because producing an accurate one by hand is the 15–40
//! minute step this tool exists to delete. Everything downstream — reduction,
//! enhancement — is cheap once it exists.
//!
//! # The profile
//!
//! Each star contributes a disc of radius `k × FWHM` at full strength, falling to
//! zero over a feather band. The falloff is **smoothstep**, not linear: a linear
//! ramp has a discontinuous derivative at both ends, and in a 16-bit mask pushed
//! through a Photoshop curve those corners show up as visible rings around every
//! star. Smoothstep is C¹ at both ends, so the seam has nothing to catch on.
//!
//! Stars combine by **maximum**, not by sum. Two overlapping stars must not add
//! up to 2.0 and clip to a flat-topped blob with a hard edge where the sum
//! crosses 1.0 — the mask is a coverage fraction, and coverage does not add.
//!
//! # Saturated stars
//!
//! A clipped core has no measurable FWHM: the profile is flat where the
//! information was, so `k × FWHM` describes the *hole*, not the star. Saturated
//! stars therefore get their radius scaled by
//! [`MaskParams::saturated_halo_k`] — FR-4's "halo extension" — because what
//! needs covering is the bloom around the core, which is far wider than the core
//! itself.

use crate::detect::Detection;
use crate::types::{Star, Tier};
use crate::Error;

/// Mask-building parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskParams {
    /// Full-strength radius as a multiple of each star's FWHM.
    pub radius_k: f64,
    /// Feather width, also as a multiple of FWHM. The mask reaches zero at
    /// `(radius_k + feather_k) × FWHM`.
    pub feather_k: f64,
    /// Extra radius multiplier for saturated stars (FR-4's halo extension).
    pub saturated_halo_k: f64,
}

impl Default for MaskParams {
    /// `radius_k = 1.0`, `feather_k = 1.0`, saturated halo ×2.
    ///
    /// A star's visible extent is a couple of FWHM, not one: at 1 × FWHM from the
    /// centre a Gaussian still holds ~6 % of its peak, and a Moffat's wings reach
    /// much further. So the full-strength core is 1 × FWHM and the mask fades out
    /// by 2 × FWHM, which covers the star without swallowing its neighbours at the
    /// 8 px separations these fields actually have.
    fn default() -> Self {
        Self {
            radius_k: 1.0,
            feather_k: 1.0,
            saturated_halo_k: 2.0,
        }
    }
}

/// A single-channel mask in `[0, 1]`, pixel-aligned to its source image.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    /// Row-major coverage, `width * height`.
    pub data: Vec<f32>,
}

impl Mask {
    pub fn zeros(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Union with another mask, by maximum. Shapes must match.
    fn union_with(&mut self, other: &Mask) {
        for (a, b) in self.data.iter_mut().zip(&other.data) {
            if *b > *a {
                *a = *b;
            }
        }
    }
}

/// The three tier masks plus their union (FR-4).
#[derive(Debug, Clone)]
pub struct TieredMasks {
    pub large: Mask,
    pub medium: Mask,
    pub small: Mask,
    /// Union of the three — the mask most users want.
    pub combined: Mask,
}

/// What the mask builder needs from a star. Deliberately narrower than either
/// `Star` or `Detection`: a mask depends on where a star is, how wide it is,
/// whether it is clipped, and its tier — and on nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskStar {
    pub x: f64,
    pub y: f64,
    pub fwhm: f64,
    pub saturated: bool,
    pub tier: Tier,
}

impl From<&Star> for MaskStar {
    fn from(s: &Star) -> Self {
        Self {
            x: s.x,
            y: s.y,
            fwhm: s.fwhm,
            saturated: s.saturated,
            tier: s.tier,
        }
    }
}

impl From<&Detection> for MaskStar {
    fn from(d: &Detection) -> Self {
        Self {
            x: d.x,
            y: d.y,
            fwhm: d.fwhm,
            saturated: d.saturated,
            tier: d.tier,
        }
    }
}

/// Smoothstep: 0 at `t <= 0`, 1 at `t >= 1`, C¹ at both ends.
#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Coverage of one star at distance `r` from its centre.
#[inline]
fn falloff(r: f64, core: f64, outer: f64) -> f64 {
    if r <= core {
        1.0
    } else if r >= outer {
        0.0
    } else {
        // 1 at the core edge, 0 at the outer edge.
        smoothstep((outer - r) / (outer - core))
    }
}

/// The full-strength and zero radii for one star.
fn radii(s: &MaskStar, p: &MaskParams) -> (f64, f64) {
    let scale = if s.saturated { p.saturated_halo_k } else { 1.0 };
    let core = (p.radius_k * s.fwhm * scale).max(0.0);
    let outer = core + (p.feather_k * s.fwhm * scale).max(0.0);
    // A zero-width band would put a hard step in the mask; keep a floor of half a
    // pixel so the edge is always resolvable.
    (core, outer.max(core + 0.5))
}

/// Build a mask from a set of stars.
pub fn build(
    stars: &[MaskStar],
    width: usize,
    height: usize,
    params: &MaskParams,
) -> Result<Mask, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    if !(params.radius_k.is_finite() && params.radius_k >= 0.0) {
        return Err(Error::InvalidParam(
            "radius_k must be finite and non-negative",
        ));
    }
    if !(params.feather_k.is_finite() && params.feather_k >= 0.0) {
        return Err(Error::InvalidParam(
            "feather_k must be finite and non-negative",
        ));
    }
    if !(params.saturated_halo_k.is_finite() && params.saturated_halo_k > 0.0) {
        return Err(Error::InvalidParam(
            "saturated_halo_k must be finite and positive",
        ));
    }

    let mut m = Mask::zeros(width, height);
    for s in stars {
        if !(s.x.is_finite() && s.y.is_finite() && s.fwhm.is_finite() && s.fwhm > 0.0) {
            // A star with no usable width cannot be masked; skipping it is honest,
            // and guessing a radius for it would put an arbitrary blob on the
            // photographer's image.
            continue;
        }
        let (core, outer) = radii(s, params);
        let x0 = ((s.x - outer).floor().max(0.0)) as usize;
        let x1 = ((s.x + outer).ceil().min((width - 1) as f64)) as usize;
        let y0 = ((s.y - outer).floor().max(0.0)) as usize;
        let y1 = ((s.y + outer).ceil().min((height - 1) as f64)) as usize;
        if s.x + outer < 0.0
            || s.y + outer < 0.0
            || s.x - outer > width as f64
            || s.y - outer > height as f64
        {
            continue;
        }

        for y in y0..=y1 {
            for x in x0..=x1 {
                let r = ((x as f64 - s.x).powi(2) + (y as f64 - s.y).powi(2)).sqrt();
                let v = falloff(r, core, outer) as f32;
                let slot = &mut m.data[y * width + x];
                // Union by maximum: coverage does not add.
                if v > *slot {
                    *slot = v;
                }
            }
        }
    }
    Ok(m)
}

/// Build the three tier masks and their union in one pass over the population.
pub fn build_tiered(
    stars: &[MaskStar],
    width: usize,
    height: usize,
    params: &MaskParams,
) -> Result<TieredMasks, Error> {
    let of_tier =
        |t: Tier| -> Vec<MaskStar> { stars.iter().copied().filter(|s| s.tier == t).collect() };
    let large = build(&of_tier(Tier::Large), width, height, params)?;
    let medium = build(&of_tier(Tier::Medium), width, height, params)?;
    let small = build(&of_tier(Tier::Small), width, height, params)?;

    // The union of the three tier masks, not a fourth independent rendering:
    // building it separately would let it drift from its own parts.
    let mut combined = large.clone();
    combined.union_with(&medium);
    combined.union_with(&small);

    Ok(TieredMasks {
        large,
        medium,
        small,
        combined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(x: f64, y: f64, fwhm: f64) -> MaskStar {
        MaskStar {
            x,
            y,
            fwhm,
            saturated: false,
            tier: Tier::Medium,
        }
    }

    #[test]
    fn a_star_is_fully_covered_at_its_centre() {
        let m = build(&[star(32.0, 32.0, 3.2)], 64, 64, &MaskParams::default()).expect("build");
        assert_eq!(m.at(32, 32), 1.0);
    }

    #[test]
    fn coverage_falls_to_zero_beyond_the_feather() {
        let p = MaskParams::default();
        let m = build(&[star(32.0, 32.0, 3.2)], 64, 64, &p).expect("build");
        // outer = (1.0 + 1.0) * 3.2 = 6.4 px
        assert_eq!(m.at(32 + 7, 32), 0.0, "mask extends past its outer radius");
        assert!(m.at(32 + 3, 32) > 0.0, "mask does not reach the core edge");
    }

    #[test]
    fn coverage_is_monotonically_decreasing_with_radius() {
        let m = build(&[star(32.0, 32.0, 4.0)], 64, 64, &MaskParams::default()).expect("build");
        let mut last = f32::INFINITY;
        for dx in 0..12 {
            let v = m.at(32 + dx, 32);
            assert!(v <= last + 1e-6, "coverage rose at dx={dx}: {v} > {last}");
            last = v;
        }
    }

    /// The reason for smoothstep: a linear ramp leaves a corner in the derivative
    /// at both ends of the feather, which shows as a ring once the mask is pushed
    /// through a curve in Photoshop.
    #[test]
    fn the_feather_has_no_kink_at_either_end() {
        let m = build(&[star(64.0, 64.0, 8.0)], 128, 128, &MaskParams::default()).expect("build");
        // Second difference along a radius must stay small — no step, no corner.
        let row: Vec<f64> = (64..90).map(|x| m.at(x, 64) as f64).collect();
        for w in row.windows(3) {
            let second = (w[2] - 2.0 * w[1] + w[0]).abs();
            assert!(
                second < 0.12,
                "kink in the feather: second difference {second}"
            );
        }
    }

    #[test]
    fn overlapping_stars_union_rather_than_sum() {
        let p = MaskParams::default();
        // Two stars 1 px apart: a sum would reach ~2.0 and clip flat.
        let m = build(&[star(32.0, 32.0, 3.2), star(33.0, 32.0, 3.2)], 64, 64, &p).expect("build");
        assert!(
            m.data.iter().all(|v| *v <= 1.0),
            "coverage exceeded 1.0 — stars are being summed"
        );
        assert_eq!(m.at(32, 32), 1.0);
    }

    #[test]
    fn a_saturated_star_gets_a_wider_halo() {
        let p = MaskParams::default();
        let plain = build(&[star(64.0, 64.0, 3.2)], 128, 128, &p).expect("build");
        let mut sat = star(64.0, 64.0, 3.2);
        sat.saturated = true;
        let halo = build(&[sat], 128, 128, &p).expect("build");

        let extent = |m: &Mask| (0..64).filter(|&dx| m.at(64 + dx, 64) > 0.0).count();
        assert!(
            extent(&halo) > extent(&plain),
            "saturated halo ({}) is not wider than a plain star ({})",
            extent(&halo),
            extent(&plain)
        );
    }

    #[test]
    fn mask_is_pixel_aligned_to_the_source() {
        let m = build(&[star(10.0, 20.0, 3.0)], 133, 71, &MaskParams::default()).expect("build");
        assert_eq!((m.width, m.height), (133, 71));
        assert_eq!(m.data.len(), 133 * 71);
    }

    #[test]
    fn a_star_at_the_frame_edge_is_clipped_not_wrapped() {
        let m = build(&[star(1.0, 1.0, 3.2)], 64, 64, &MaskParams::default()).expect("build");
        assert_eq!(m.at(1, 1), 1.0);
        // Nothing may appear at the opposite corner.
        assert_eq!(m.at(63, 63), 0.0, "mask wrapped around the frame");
    }

    #[test]
    fn a_star_outside_the_frame_contributes_nothing_and_does_not_panic() {
        let m = build(&[star(-50.0, -50.0, 3.2)], 64, 64, &MaskParams::default()).expect("build");
        assert!(m.data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_star_with_no_usable_width_is_skipped_rather_than_guessed() {
        let p = MaskParams::default();
        for bad in [0.0, f64::NAN, -1.0] {
            let m = build(&[star(32.0, 32.0, bad)], 64, 64, &p).expect("build");
            assert!(
                m.data.iter().all(|v| *v == 0.0),
                "fwhm {bad} produced a blob"
            );
        }
    }

    #[test]
    fn an_empty_catalog_yields_an_empty_mask() {
        let m = build(&[], 64, 64, &MaskParams::default()).expect("build");
        assert!(m.data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn tiers_are_separated_and_the_combined_mask_is_their_union() {
        let mut a = star(20.0, 20.0, 3.2);
        a.tier = Tier::Large;
        let mut b = star(40.0, 40.0, 3.2);
        b.tier = Tier::Medium;
        let mut c = star(60.0, 60.0, 3.2);
        c.tier = Tier::Small;
        let t = build_tiered(&[a, b, c], 80, 80, &MaskParams::default()).expect("build");

        assert_eq!(t.large.at(20, 20), 1.0);
        assert_eq!(
            t.large.at(40, 40),
            0.0,
            "a medium star leaked into the large mask"
        );
        assert_eq!(t.medium.at(40, 40), 1.0);
        assert_eq!(t.small.at(60, 60), 1.0);

        for (i, v) in t.combined.data.iter().enumerate() {
            let want = t.large.data[i].max(t.medium.data[i]).max(t.small.data[i]);
            assert_eq!(*v, want, "combined is not the union of the tiers at {i}");
        }
    }

    #[test]
    fn building_is_deterministic() {
        let stars: Vec<MaskStar> = (0..50)
            .map(|i| {
                star(
                    5.0 + (i % 10) as f64 * 12.0,
                    5.0 + (i / 10) as f64 * 12.0,
                    3.2,
                )
            })
            .collect();
        let a = build(&stars, 128, 128, &MaskParams::default()).expect("a");
        let b = build(&stars, 128, 128, &MaskParams::default()).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn star_order_does_not_change_the_mask() {
        // Union by maximum is commutative; if it ever stops being, this catches it.
        let s: Vec<MaskStar> = vec![
            star(30.0, 32.0, 3.2),
            star(33.0, 32.0, 4.0),
            star(31.0, 34.0, 2.5),
        ];
        let mut r: Vec<MaskStar> = s.clone();
        r.reverse();
        let a = build(&s, 64, 64, &MaskParams::default()).expect("a");
        let b = build(&r, 64, 64, &MaskParams::default()).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_invalid_params() {
        let bad = MaskParams {
            radius_k: -1.0,
            ..Default::default()
        };
        assert!(build(&[star(1.0, 1.0, 3.0)], 8, 8, &bad).is_err());
    }

    #[test]
    fn rejects_an_empty_image() {
        assert!(matches!(
            build(&[], 0, 0, &MaskParams::default()).expect_err("must fail"),
            Error::EmptyImage
        ));
    }
}
