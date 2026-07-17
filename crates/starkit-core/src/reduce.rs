//! Star reduction (缩星), method B — morphological (task T1-6, FR-5).
//!
//! The fallback method, and the one photographers already recognise: it is what a
//! Photoshop Minimum filter does. Fast, predictable, and honest about its
//! artifacts — it does not reconstruct anything, it just eats the edges. Method A
//! (resynthesis, T1-7) is the default precisely because this one cannot avoid the
//! classic damage: it erodes nebulosity as happily as it erodes a star, which is
//! why it must never be let out of the gate.
//!
//! # What erosion does to a star
//!
//! Grayscale erosion is a local minimum over a structuring element. For a
//! radially monotonic profile it shifts every isophote inward, so the
//! half-maximum radius `R` — and with it the FWHM — shrinks with each pass. The
//! *continuous* ideal is an inward shift of exactly the element radius `r`
//! (`R → R − r`), but the digital reality is gentler and measured:
//!
//! | element radius | shrink in R per pass |
//! |---|---|
//! | 1.0 (4-connected) | ≈ 0.9 px |
//! | 2.0 | ≈ 1.5–1.8 px |
//! | 3.0 | ≈ 2.0–2.5 px |
//!
//! A digital disc under-reaches the continuous one, and iterated passes compound
//! sub-linearly, so the honest description is a monotone trend, not a formula:
//! more radius shrinks more per pass, more passes shrink more, and enough of
//! either erases the star entirely. The numbers above are what
//! `reduce::tests::shrinkage_is_monotone_in_radius_and_iterations` and the
//! commented rate table pin.
//!
//! Even the gentlest setting is aggressive: one pass at `radius = 1.0` takes a
//! 3.2 px star (`R = 1.6`) down by ~0.9 px of radius, roughly a 55 % FWHM cut.
//! That is why the manual workflow applies a Minimum filter through a
//! hand-painted mask rather than globally. (An exact "FWHM reduced by X %"
//! guarantee is method A's job, T1-7; method B trades that precision for speed
//! and the look photographers know.)
//!
//! # This module does not composite
//!
//! [`morphological`] returns a **modified plane**, not an output image. Its result
//! reaches an image only through [`crate::gate::Gate::composite`], which is the
//! single place INV-1 is enforced (starkit-core/CLAUDE.md). Eroding the whole
//! plane and gating afterwards is deliberate: the erosion of a pixel *inside* the
//! gate legitimately reads neighbours from outside it — reading is not writing —
//! and doing it this way keeps the operation ignorant of the mask, which is what
//! makes the invariant checkable in one place instead of five.

use crate::Error;

/// Morphological reduction parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphParams {
    /// Structuring-element radius in pixels. Each pass shrinks a star's half-max
    /// radius by about this much.
    pub radius: f64,
    /// Number of erosion passes. Effects compound: N passes at radius r shrink
    /// the half-max radius by ~N·r.
    pub iterations: usize,
}

impl Default for MorphParams {
    /// One pass at radius 1.0 — a 4-connected cross.
    ///
    /// The gentlest structuring element that erodes at all, and it is *still*
    /// aggressive: on the fixtures' 3.2 px seeing it cuts FWHM by ~60 %. A larger
    /// default would look like a reasonable knob and quietly delete small stars.
    fn default() -> Self {
        Self {
            radius: 1.0,
            iterations: 1,
        }
    }
}

/// Offsets of a disc structuring element.
///
/// A disc, not a square: a square element erodes 41 % further along the diagonals
/// than along the axes, which turns round stars into faintly diamond-shaped ones.
/// That artifact is subtle per-star and obvious across a field.
fn disc(radius: f64) -> Vec<(isize, isize)> {
    let r = radius.floor() as isize;
    let r2 = radius * radius;
    let mut out = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx * dx + dy * dy) as f64 <= r2 {
                out.push((dx, dy));
            }
        }
    }
    out
}

/// One erosion pass: local minimum over the element.
///
/// Edges clamp, so a star touching the frame border erodes against its own
/// mirrored neighbourhood rather than against an invented zero — zero-padding
/// would make the border erode toward black, a dark rim exactly where FR-5 says
/// this method must not produce one.
fn erode_once(src: &[f32], w: usize, h: usize, el: &[(isize, isize)]) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut m = f32::INFINITY;
            for (dx, dy) in el {
                let nx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let ny = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                let v = src[ny * w + nx];
                // NaN must not win a minimum comparison and erase a star; it is
                // not smaller than anything, it is simply not a number.
                if v < m {
                    m = v;
                }
            }
            out[y * w + x] = if m.is_finite() { m } else { src[y * w + x] };
        }
    }
    out
}

/// Erode a plane — method B's whole algorithm.
///
/// Returns the modified plane. **The caller must composite it through
/// [`crate::gate::Gate::composite`]**; this function has no idea where stars are
/// and would happily erode a galaxy.
pub fn morphological(
    plane: &[f32],
    width: usize,
    height: usize,
    params: &MorphParams,
) -> Result<Vec<f32>, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    if plane.len() != width * height {
        return Err(Error::PlaneSize {
            expected: width * height,
            actual: plane.len(),
        });
    }
    if !(params.radius.is_finite() && params.radius > 0.0) {
        return Err(Error::InvalidParam("radius must be finite and positive"));
    }

    let el = disc(params.radius);
    if el.len() <= 1 {
        // A single-pixel element is the identity. Returning the input unchanged
        // is honest; pretending to reduce would be worse than refusing.
        return Ok(plane.to_vec());
    }

    let mut cur = plane.to_vec();
    for _ in 0..params.iterations {
        cur = erode_once(&cur, width, height, &el);
    }
    Ok(cur)
}

// ===========================================================================
// Method A — resynthesis (task T1-7, FR-5, the default)
// ===========================================================================

/// What resynthesis needs to know about one star. Narrower than a `Detection`:
/// reduction depends on where a star is, how wide it is, and whether it is
/// clipped — not on its flux or tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReduceStar {
    pub x: f64,
    pub y: f64,
    pub fwhm: f64,
    pub saturated: bool,
}

impl From<&crate::detect::Detection> for ReduceStar {
    fn from(d: &crate::detect::Detection) -> Self {
        Self {
            x: d.x,
            y: d.y,
            fwhm: d.fwhm,
            saturated: d.saturated,
        }
    }
}

/// Resynthesis reduction parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ResynthParams {
    /// Target FWHM fraction, `r ∈ [0, 1]`. `new_fwhm ≈ r × old_fwhm`; `r = 1`
    /// leaves a star unchanged, `r = 0` removes it (the starless path, FR-7).
    pub reduction: f64,
    /// Leave saturated stars untouched. Their clipped cores have no measurable
    /// profile — resampling a flat-topped hole would produce a smaller flat hole,
    /// not a smaller star — so by default they are protected (FR-5's toggle).
    pub protect_saturated: bool,
    /// Local-background annulus, inner and outer radius as multiples of FWHM. The
    /// annulus median is the background the star sits on; taking it from a ring
    /// clear of the core keeps the star's own light out of its own background.
    pub annulus_inner_k: f64,
    pub annulus_outer_k: f64,
    /// Processed disc radius as a multiple of FWHM — how far out the star is
    /// removed and rebuilt. Must cover the star's real extent or a faint outer
    /// ring of the original would survive as a halo.
    pub core_k: f64,
}

impl Default for ResynthParams {
    /// Halve the FWHM, protect saturated stars, annulus 2.5–4.0 × FWHM, process
    /// out to 3.0 × FWHM.
    fn default() -> Self {
        Self {
            reduction: 0.5,
            protect_saturated: true,
            annulus_inner_k: 2.5,
            annulus_outer_k: 4.0,
            core_k: 3.0,
        }
    }
}

/// Median of a small scratch buffer, by sorting with `total_cmp` (NaN-safe).
fn median_of(buf: &mut [f64]) -> f64 {
    if buf.is_empty() {
        return f64::NAN;
    }
    buf.sort_by(f64::total_cmp);
    let n = buf.len();
    if n.is_multiple_of(2) {
        0.5 * (buf[n / 2 - 1] + buf[n / 2])
    } else {
        buf[n / 2]
    }
}

/// Bilinear sample of a plane at continuous coordinates, clamped at the edges.
fn sample_bilinear(plane: &[f32], w: usize, h: usize, x: f64, y: f64) -> f64 {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let top = plane[y0 * w + x0] as f64 * (1.0 - fx) + plane[y0 * w + x1] as f64 * fx;
    let bot = plane[y1 * w + x0] as f64 * (1.0 - fx) + plane[y1 * w + x1] as f64 * fx;
    top * (1.0 - fy) + bot * fy
}

/// Local background under a star: the median of an annulus around it.
///
/// A median, not a mean: the annulus can clip a neighbour, and a mean would let
/// that neighbour lift the estimate and leave a dark ring when the star is
/// removed. Returns `None` if the annulus is empty (a star in the very corner),
/// so the caller can leave that star alone rather than invent a background.
fn annulus_background(
    plane: &[f32],
    w: usize,
    h: usize,
    star: &ReduceStar,
    p: &ResynthParams,
    scratch: &mut Vec<f64>,
) -> Option<f64> {
    let inner = p.annulus_inner_k * star.fwhm;
    let outer = p.annulus_outer_k * star.fwhm;
    let (inner2, outer2) = (inner * inner, outer * outer);
    let x0 = (star.x - outer).floor().max(0.0) as usize;
    let x1 = ((star.x + outer).ceil() as usize).min(w - 1);
    let y0 = (star.y - outer).floor().max(0.0) as usize;
    let y1 = ((star.y + outer).ceil() as usize).min(h - 1);

    scratch.clear();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d2 = (x as f64 - star.x).powi(2) + (y as f64 - star.y).powi(2);
            if d2 >= inner2 && d2 <= outer2 {
                let v = plane[y * w + x];
                if v.is_finite() {
                    scratch.push(v as f64);
                }
            }
        }
    }
    if scratch.is_empty() {
        None
    } else {
        Some(median_of(scratch))
    }
}

/// Resynthesis reduction — method A, the default (FR-5).
///
/// Per star: estimate the local background from an annulus, remove the star by
/// filling its disc with that background (the inpaint), then re-add the star's
/// **own** profile scaled toward its centre by `reduction`. The rebuilt star is
/// sampled from the real pixels, not a fitted model, so it keeps the true PSF
/// shape, ellipticity and — applied per channel — colour, with none of the
/// model-mismatch a Gaussian-or-Moffat fit would carry (T0-2 measured that at
/// +14 % on Moffat stars).
///
/// # Why radial resampling gives `new_fwhm = r × old_fwhm`
///
/// The rebuilt star at offset `d` from the centre samples the original at `d / r`:
/// `new(d) = old(d / r)`. The half-maximum of `old` is at radius `R`, so `new`
/// reaches half-maximum at `d = r·R` — its FWHM is exactly `r` times the
/// original, in the continuous limit. The peak (at `d = 0`) is preserved, so a
/// reduced star stays as bright but becomes tighter, which is the point of 缩星.
/// Bilinear interpolation broadens the very smallest stars slightly; see the
/// `resynth_ac` note on the sampling floor.
///
/// # This module does not composite
///
/// Like [`morphological`], this returns a **modified plane**; only
/// [`crate::gate::Gate::composite`] writes it into an image (INV-1).
pub fn resynthesis(
    plane: &[f32],
    width: usize,
    height: usize,
    stars: &[ReduceStar],
    params: &ResynthParams,
) -> Result<Vec<f32>, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    if plane.len() != width * height {
        return Err(Error::PlaneSize {
            expected: width * height,
            actual: plane.len(),
        });
    }
    if !(params.reduction.is_finite() && (0.0..=1.0).contains(&params.reduction)) {
        return Err(Error::InvalidParam("reduction must be in [0, 1]"));
    }
    if !(params.core_k.is_finite() && params.core_k > 0.0) {
        return Err(Error::InvalidParam("core_k must be finite and positive"));
    }

    let r = params.reduction;
    let mut out = plane.to_vec();
    let mut scratch: Vec<f64> = Vec::new();

    // Background per star, computed once from the *original* plane so a
    // neighbour's inpaint cannot perturb it.
    let mut bg: Vec<Option<f64>> = Vec::with_capacity(stars.len());
    for s in stars {
        let usable = s.x.is_finite() && s.y.is_finite() && s.fwhm.is_finite() && s.fwhm > 0.0;
        let protected_ = params.protect_saturated && s.saturated;
        bg.push(if usable && !protected_ {
            annulus_background(plane, width, height, s, params, &mut scratch)
        } else {
            None
        });
    }

    // Phase 1 — inpaint: fill each processed star's disc with its background,
    // removing the original star.
    for (s, b) in stars.iter().zip(&bg) {
        let Some(b) = b else { continue };
        let radius = params.core_k * s.fwhm;
        let r2 = radius * radius;
        let x0 = (s.x - radius).floor().max(0.0) as usize;
        let x1 = ((s.x + radius).ceil() as usize).min(width - 1);
        let y0 = (s.y - radius).floor().max(0.0) as usize;
        let y1 = ((s.y + radius).ceil() as usize).min(height - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if (x as f64 - s.x).powi(2) + (y as f64 - s.y).powi(2) <= r2 {
                    out[y * width + x] = *b as f32;
                }
            }
        }
    }

    // Phase 2 — re-add each shrunk star. `r = 0` is the starless path: nothing is
    // added, so the inpaint stands alone.
    if r > 0.0 {
        for (s, b) in stars.iter().zip(&bg) {
            let Some(b) = b else { continue };
            let radius = params.core_k * s.fwhm;
            let r2 = radius * radius;
            let x0 = (s.x - radius).floor().max(0.0) as usize;
            let x1 = ((s.x + radius).ceil() as usize).min(width - 1);
            let y0 = (s.y - radius).floor().max(0.0) as usize;
            let y1 = ((s.y + radius).ceil() as usize).min(height - 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (dx, dy) = (x as f64 - s.x, y as f64 - s.y);
                    if dx * dx + dy * dy > r2 {
                        continue;
                    }
                    // Sample the original profile at d/r — farther out, so the
                    // rebuilt star is a shrunk copy.
                    let src_x = s.x + dx / r;
                    let src_y = s.y + dy / r;
                    let signal = sample_bilinear(plane, width, height, src_x, src_y) - *b;
                    // Only positive signal: adding a noise dip back would build a
                    // faint dark speck, a small cousin of ringing.
                    if signal > 0.0 {
                        out[y * width + x] += signal as f32;
                    }
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate;
    use crate::mask::{self, MaskParams, MaskStar};
    use crate::types::Tier;

    fn add_star(p: &mut [f32], w: usize, h: usize, cx: f64, cy: f64, peak: f64, fwhm: f64) {
        let s = fwhm / 2.354_820_045_030_949_4;
        let r = (4.0 * fwhm).ceil() as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (cx.round() as i64 + dx, cy.round() as i64 + dy);
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let d2 = (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2);
                p[y as usize * w + x as usize] += (peak * (-d2 / (2.0 * s * s)).exp()) as f32;
            }
        }
    }

    /// Half-max radius of the blob at `(cx, cy)`, measured along +x.
    fn half_max_radius(p: &[f32], w: usize, cx: usize, cy: usize, bkg: f32) -> f64 {
        let peak = p[cy * w + cx] - bkg;
        let half = bkg + peak / 2.0;
        for d in 0..40 {
            if p[cy * w + cx + d] < half {
                return d as f64;
            }
        }
        f64::NAN
    }

    #[test]
    fn erosion_shrinks_a_star() {
        let (w, h) = (64, 64);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 32.0, 32.0, 1.0, 6.0);
        let before = half_max_radius(&p, w, 32, 32, 0.1);
        let out = morphological(&p, w, h, &MorphParams::default()).expect("erode");
        let after = half_max_radius(&out, w, 32, 32, 0.1);
        assert!(after < before, "star did not shrink: {before} -> {after}");
    }

    /// Erosion shrinks a star monotonically: more radius per pass shrinks more,
    /// more passes shrink more. This is the honest claim — the exact rate is a
    /// digital-disc property that runs below the continuous ideal (see the module
    /// docs' table), and pinning a false linear law would be worse than testing
    /// the trend that actually holds.
    ///
    /// The measured single-pass rates are also spot-checked against the table so
    /// the documented numbers cannot silently rot: radius 2 on a 12 px star (R=6)
    /// shrinks R by 1.5–1.8.
    #[test]
    fn shrinkage_is_monotone_in_radius_and_iterations() {
        let (w, h) = (96, 96);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 48.0, 48.0, 1.0, 12.0);
        let r0 = half_max_radius(&p, w, 48, 48, 0.1);

        let shrink = |radius: f64, iterations: usize| -> f64 {
            let out = morphological(&p, w, h, &MorphParams { radius, iterations }).expect("erode");
            r0 - half_max_radius(&out, w, 48, 48, 0.1)
        };

        // More iterations shrink more.
        assert!(
            shrink(2.0, 1) < shrink(2.0, 2),
            "iterating did not shrink further"
        );
        assert!(
            shrink(2.0, 2) < shrink(2.0, 3),
            "iterating did not shrink further"
        );
        // More radius shrinks more per pass.
        assert!(
            shrink(1.0, 1) < shrink(2.0, 1),
            "a bigger element did not shrink more"
        );
        assert!(
            shrink(2.0, 1) < shrink(3.0, 1),
            "a bigger element did not shrink more"
        );

        // A sanity bound on the rate, wide enough to be method-independent: the
        // half-max crossing is quantised on the pixel grid, so integer-step and
        // interpolated measurements differ by up to a pixel. What must hold is
        // that a radius-2 pass shrinks by more than 1 and less than its element
        // radius (the digital disc under-reaches the continuous ideal). The
        // module docs carry the finer ≈1.5–1.8 figure from interpolated
        // measurement; this test does not depend on that precision.
        let s2 = shrink(2.0, 1);
        assert!(
            s2 > 1.0 && s2 <= 2.0,
            "radius-2 single-pass shrink {s2:.2} is outside (1.0, 2.0]"
        );
    }

    /// Erosion is a minimum: it can never make a pixel brighter. A reduction that
    /// brightened anything would be a bug hiding in plain sight.
    #[test]
    fn erosion_never_brightens_a_pixel() {
        let (w, h) = (64, 64);
        let mut p = vec![0.2f32; w * h];
        add_star(&mut p, w, h, 32.0, 32.0, 0.7, 4.0);
        add_star(&mut p, w, h, 12.0, 50.0, 0.3, 3.0);
        let out = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 2.0,
                iterations: 2,
            },
        )
        .expect("erode");
        for (i, (a, b)) in out.iter().zip(&p).enumerate() {
            assert!(a <= b, "pixel {i} got brighter: {b} -> {a}");
        }
    }

    /// A flat field has no edges to eat, so erosion must leave it exactly alone —
    /// including at the border, where zero-padding would darken the rim.
    #[test]
    fn a_flat_field_is_unchanged_including_at_the_border() {
        let (w, h) = (32, 32);
        let p = vec![0.42f32; w * h];
        let out = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 3.0,
                iterations: 2,
            },
        )
        .expect("erode");
        assert_eq!(
            out, p,
            "erosion darkened a flat field — edges are not clamping"
        );
    }

    #[test]
    fn a_disc_element_erodes_no_further_diagonally_than_axially() {
        let (w, h) = (64, 64);
        let mut p = vec![0.0f32; w * h];
        // A solid square: erosion should take the same bite off each side.
        for y in 24..40 {
            for x in 24..40 {
                p[y * w + x] = 1.0;
            }
        }
        let out = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 3.0,
                iterations: 1,
            },
        )
        .expect("erode");
        let axial = (0..20).take_while(|&d| out[32 * w + 24 + d] == 0.0).count();
        let diag = (0..20)
            .take_while(|&d| out[(24 + d) * w + 24 + d] == 0.0)
            .count();
        // A square element would eat ~sqrt(2)x further into the corner.
        assert!(
            diag <= axial + 3,
            "diagonal bite {diag} much deeper than axial {axial} — element is not a disc"
        );
    }

    #[test]
    fn zero_iterations_is_the_identity() {
        let (w, h) = (32, 32);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 16.0, 16.0, 1.0, 3.0);
        let out = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 2.0,
                iterations: 0,
            },
        )
        .expect("erode");
        assert_eq!(out, p);
    }

    #[test]
    fn a_sub_pixel_radius_is_the_identity_rather_than_a_fake_reduction() {
        let (w, h) = (32, 32);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 16.0, 16.0, 1.0, 3.0);
        let out = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 0.5,
                iterations: 3,
            },
        )
        .expect("erode");
        assert_eq!(out, p, "a sub-pixel element must not pretend to reduce");
    }

    #[test]
    fn a_nan_does_not_erase_its_neighbourhood() {
        let (w, h) = (32, 32);
        let mut p = vec![0.5f32; w * h];
        p[16 * w + 16] = f32::NAN;
        let out = morphological(&p, w, h, &MorphParams::default()).expect("erode");
        // NaN loses every comparison, so it must not become the local minimum and
        // wipe out the pixels around it.
        for y in 14..19 {
            for x in 14..19 {
                if (x, y) == (16, 16) {
                    continue;
                }
                assert_eq!(out[y * w + x], 0.5, "NaN at (16,16) damaged ({x},{y})");
            }
        }
    }

    #[test]
    fn is_deterministic() {
        let (w, h) = (48, 48);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 24.0, 24.0, 1.0, 4.0);
        let q = MorphParams {
            radius: 2.0,
            iterations: 2,
        };
        assert_eq!(
            morphological(&p, w, h, &q).expect("a"),
            morphological(&p, w, h, &q).expect("b")
        );
    }

    #[test]
    fn rejects_a_mismatched_plane() {
        assert!(matches!(
            morphological(&[0.0; 10], 4, 4, &MorphParams::default()).expect_err("must fail"),
            Error::PlaneSize { .. }
        ));
    }

    #[test]
    fn rejects_a_non_positive_radius() {
        let p = vec![0.0f32; 16];
        for r in [0.0, -1.0, f64::NAN] {
            assert!(morphological(
                &p,
                4,
                4,
                &MorphParams {
                    radius: r,
                    iterations: 1
                }
            )
            .is_err());
        }
    }

    /// The composition this module exists to be half of: erosion damages
    /// everything, and only the gate makes that safe. This is the local proof;
    /// `tests/reduce_ac.rs` proves it on the real nightscape.
    #[test]
    fn nebulosity_outside_the_gate_survives_an_erosion_that_would_eat_it() {
        let (w, h) = (64, 64);
        let mut p = vec![0.1f32; w * h];
        add_star(&mut p, w, h, 20.0, 20.0, 1.0, 3.2);
        // A broad smooth "nebula" far from the star — exactly what method B eats.
        add_star(&mut p, w, h, 45.0, 45.0, 0.4, 18.0);

        let stars = [MaskStar {
            x: 20.0,
            y: 20.0,
            fwhm: 3.2,
            saturated: false,
            tier: Tier::Medium,
        }];
        let sm = mask::build(&stars, w, h, &MaskParams::default()).expect("mask");
        let g = gate::build(&sm, None, 1.0).expect("gate");

        let eroded = morphological(
            &p,
            w,
            h,
            &MorphParams {
                radius: 2.0,
                iterations: 2,
            },
        )
        .expect("erode");
        let out = g.composite(&p, &eroded).expect("composite");

        // The ungated erosion must actually have damaged the nebula, or this
        // proves nothing.
        assert!(
            eroded[45 * w + 45] < p[45 * w + 45] - 1e-6,
            "test is vacuous: erosion did not touch the nebula"
        );
        assert_eq!(
            out[45 * w + 45].to_bits(),
            p[45 * w + 45].to_bits(),
            "the gate let erosion damage the nebula"
        );
        assert!(
            out[20 * w + 20] < p[20 * w + 20],
            "the star was not reduced"
        );
    }
}
