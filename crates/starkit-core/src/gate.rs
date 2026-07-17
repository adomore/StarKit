//! The mask gate — INV-1's single enforcement point (task T1-5).
//!
//! **Every** star operation's output reaches an image through [`Gate::composite`]
//! and nowhere else. That is not a convention: INV-1 says pixels outside
//! `sky_mask ∧ dilate(star_mask)` are bit-identical to the input, and a guarantee
//! spread across five call sites is a guarantee nobody can check. Operations take
//! masks as arguments and return a modified plane; they never composite
//! themselves (starkit-core/CLAUDE.md).
//!
//! # Why bit-identical needs a branch, not a blend
//!
//! The natural way to write a gated blend is
//! `out = input·(1 − a) + modified·a`, which at `a = 0` looks like it returns
//! `input`. It does not, quite. `input·1.0 + modified·0.0` is `input + 0.0`, and
//! `-0.0 + 0.0 == +0.0` — a different bit pattern. If `modified` holds an inf or
//! a NaN from a divide the operation got wrong, `modified · 0.0` is NaN and the
//! poison spreads into pixels the gate was supposed to protect.
//!
//! So a zero gate **copies**. INV-1 is a bit-level claim and is implemented at the
//! bit level.
//!
//! # The foreground margin
//!
//! [`restrict_to_sky`] drops detections whose neighbourhood touches the
//! foreground. This is not belt-and-braces on top of the compositor — it answers
//! a different failure. A matched filter measures contrast against its
//! surroundings, so ordinary sky beside a black tree silhouette *is* a strong
//! response, and detection invents stars along every branch (D-019/D-034: 35.7 %
//! precision on `nightscape-fg`). Those phantom stars are inside the sky mask, so
//! the compositor would happily let a reduction operate on them. Both live here
//! because both are the sky mask's business.

use crate::detect::Detection;
use crate::mask::Mask;
use crate::Error;

/// The region star operations are permitted to touch: `sky ∧ dilate(star)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub width: usize,
    pub height: usize,
    /// Per-pixel permission in `[0, 1]`. Exactly 0.0 means "never touch".
    allowed: Vec<f32>,
}

impl Gate {
    #[inline]
    pub fn allowed_at(&self, x: usize, y: usize) -> f32 {
        self.allowed[y * self.width + x]
    }

    /// Fraction of the frame the gate opens at all. Useful for reports (FR-8).
    pub fn open_fraction(&self) -> f64 {
        self.allowed.iter().filter(|v| **v > 0.0).count() as f64 / self.allowed.len() as f64
    }

    /// Composite a modified plane over the input, under the gate.
    ///
    /// **This is the only function in the workspace permitted to put an operation's
    /// output into an image.** Outside the gate the input is copied bit-for-bit;
    /// inside, the two are blended by the gate's own value, so a feathered star
    /// mask feathers the operation with it.
    pub fn composite(&self, input: &[f32], modified: &[f32]) -> Result<Vec<f32>, Error> {
        let n = self.width * self.height;
        if input.len() != n {
            return Err(Error::PlaneSize {
                expected: n,
                actual: input.len(),
            });
        }
        if modified.len() != n {
            return Err(Error::PlaneSize {
                expected: n,
                actual: modified.len(),
            });
        }

        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let a = self.allowed[i];
            // The branch *is* the invariant. See the module docs: a blend at
            // a = 0 is not bit-identical, and would carry a NaN across the gate.
            if a == 0.0 {
                out.push(input[i]);
            } else if a >= 1.0 {
                out.push(modified[i]);
            } else {
                let a = a as f64;
                out.push((input[i] as f64 * (1.0 - a) + modified[i] as f64 * a) as f32);
            }
        }
        Ok(out)
    }
}

/// Dilate a mask by `radius` pixels, keeping the feather.
///
/// A star operation needs a little room beyond the mask edge — a reduction
/// resamples a star's wings, which reach past the core the mask was drawn around.
/// Implemented as a max-filter over a disc: dilation of a soft mask is the
/// running maximum, which preserves the feather rather than hardening it into a
/// step.
fn dilate(mask: &Mask, radius: f64) -> Vec<f32> {
    let (w, h) = (mask.width, mask.height);
    if radius <= 0.0 {
        return mask.data.clone();
    }
    let r = radius.ceil() as isize;
    let r2 = radius * radius;

    // Separable would be wrong here: a max over a square is not a max over a disc,
    // and would extend the gate into the corners by a factor of sqrt(2).
    let mut offsets: Vec<(isize, isize)> = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx * dx + dy * dy) as f64 <= r2 {
                offsets.push((dx, dy));
            }
        }
    }

    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut m = 0.0f32;
            for (dx, dy) in &offsets {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let v = mask.data[ny as usize * w + nx as usize];
                if v > m {
                    m = v;
                }
            }
            out[y * w + x] = m;
        }
    }
    out
}

/// Build the gate: `sky ∧ dilate(star, dilate_px)`.
///
/// `sky` is `None` for a deep-sky frame with no foreground — the whole frame is
/// sky. It is **not** optional in the sense of "skip the check": a `None` sky is
/// an explicit claim that every pixel is sky, and the star mask still gates.
pub fn build(star_mask: &Mask, sky: Option<&[f32]>, dilate_px: f64) -> Result<Gate, Error> {
    let (w, h) = (star_mask.width, star_mask.height);
    if w == 0 || h == 0 {
        return Err(Error::EmptyImage);
    }
    if star_mask.data.len() != w * h {
        return Err(Error::PlaneSize {
            expected: w * h,
            actual: star_mask.data.len(),
        });
    }
    if !(dilate_px.is_finite() && dilate_px >= 0.0) {
        return Err(Error::InvalidParam(
            "dilate_px must be finite and non-negative",
        ));
    }
    if let Some(s) = sky {
        if s.len() != w * h {
            // A sky mask of the wrong size is the photographer's mask for a
            // different image. Silently resampling it would gate the wrong
            // pixels, which is the one thing INV-1 exists to prevent.
            return Err(Error::PlaneSize {
                expected: w * h,
                actual: s.len(),
            });
        }
    }

    let dilated = dilate(star_mask, dilate_px);
    let allowed: Vec<f32> = match sky {
        None => dilated,
        Some(s) => dilated
            .iter()
            .zip(s)
            .map(|(d, k)| {
                // Logical AND of two soft masks is their product. A hard zero on
                // either side must survive as an exact zero, and it does: any
                // finite x times 0.0 is 0.0.
                let k = k.clamp(0.0, 1.0);
                if k == 0.0 || *d == 0.0 {
                    0.0
                } else {
                    d * k
                }
            })
            .collect(),
    };

    Ok(Gate {
        width: w,
        height: h,
        allowed,
    })
}

/// Drop detections whose neighbourhood touches the foreground (D-034).
///
/// `margin_px` should cover the detection filter's reach: a source is discarded
/// when any pixel within that radius is outside the sky. That is deliberately
/// stricter than "the centre is in the sky" — the matched filter's response at a
/// silhouette edge peaks on sky pixels a couple of px from the branch, so a
/// centre test would keep exactly the artifacts this exists to remove.
pub fn restrict_to_sky(
    detections: &[Detection],
    sky: &[f32],
    width: usize,
    height: usize,
    margin_px: f64,
) -> Result<Vec<Detection>, Error> {
    if sky.len() != width * height {
        return Err(Error::PlaneSize {
            expected: width * height,
            actual: sky.len(),
        });
    }
    let r = margin_px.max(0.0).ceil() as isize;
    let r2 = margin_px * margin_px;

    let keep = |d: &Detection| -> bool {
        let cx = d.x.round() as isize;
        let cy = d.y.round() as isize;
        for dy in -r..=r {
            for dx in -r..=r {
                if (dx * dx + dy * dy) as f64 > r2 {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
                    // Off-frame is not foreground; the frame edge is its own
                    // problem and not this function's to invent an answer for.
                    continue;
                }
                if sky[y as usize * width + x as usize] <= 0.0 {
                    return false;
                }
            }
        }
        true
    };

    Ok(detections.iter().filter(|d| keep(d)).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{MaskParams, MaskStar};
    use crate::types::Tier;

    fn star_mask(w: usize, h: usize, stars: &[(f64, f64, f64)]) -> Mask {
        let s: Vec<MaskStar> = stars
            .iter()
            .map(|&(x, y, fwhm)| MaskStar {
                x,
                y,
                fwhm,
                saturated: false,
                tier: Tier::Medium,
            })
            .collect();
        crate::mask::build(&s, w, h, &MaskParams::default()).expect("mask")
    }

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32 * 0.001).collect()
    }

    /// INV-1, stated as bluntly as it can be: outside the gate, bit-identical.
    #[test]
    fn pixels_outside_the_gate_are_bit_identical() {
        let (w, h) = (64, 64);
        let m = star_mask(w, h, &[(32.0, 32.0, 3.2)]);
        let g = build(&m, None, 2.0).expect("gate");
        let input = ramp(w * h);
        let modified = vec![999.0f32; w * h];
        let out = g.composite(&input, &modified).expect("composite");

        for i in 0..w * h {
            if g.allowed[i] == 0.0 {
                assert_eq!(
                    out[i].to_bits(),
                    input[i].to_bits(),
                    "pixel {i} outside the gate was modified"
                );
            }
        }
    }

    /// The reason the compositor branches instead of blending. A blend would
    /// return `-0.0 + 0.0 = +0.0`, a different bit pattern for the same number.
    #[test]
    fn negative_zero_survives_the_gate_unchanged() {
        let (w, h) = (16, 16);
        let m = star_mask(w, h, &[(2.0, 2.0, 2.0)]);
        let g = build(&m, None, 0.0).expect("gate");
        let mut input = vec![-0.0f32; w * h];
        input[w * h - 1] = -0.0;
        let modified = vec![5.0f32; w * h];
        let out = g.composite(&input, &modified).expect("composite");
        let last = w * h - 1;
        assert_eq!(
            g.allowed[last], 0.0,
            "test setup: corner must be outside the gate"
        );
        assert_eq!(
            out[last].to_bits(),
            (-0.0f32).to_bits(),
            "-0.0 became +0.0: the gate is blending, not copying"
        );
    }

    /// A NaN produced by a broken operation must not escape through a closed gate.
    #[test]
    fn a_nan_in_the_modified_plane_cannot_cross_a_closed_gate() {
        let (w, h) = (32, 32);
        let m = star_mask(w, h, &[(16.0, 16.0, 3.0)]);
        let g = build(&m, None, 1.0).expect("gate");
        let input = ramp(w * h);
        let modified = vec![f32::NAN; w * h];
        let out = g.composite(&input, &modified).expect("composite");
        for i in 0..w * h {
            if g.allowed[i] == 0.0 {
                assert!(!out[i].is_nan(), "NaN leaked through a closed gate at {i}");
                assert_eq!(out[i].to_bits(), input[i].to_bits());
            }
        }
    }

    #[test]
    fn a_fully_open_gate_takes_the_modified_value() {
        let (w, h) = (32, 32);
        let m = star_mask(w, h, &[(16.0, 16.0, 4.0)]);
        let g = build(&m, None, 0.0).expect("gate");
        let input = vec![1.0f32; w * h];
        let modified = vec![7.0f32; w * h];
        let out = g.composite(&input, &modified).expect("composite");
        assert_eq!(
            g.allowed_at(16, 16),
            1.0,
            "test setup: centre must be fully open"
        );
        assert_eq!(out[16 * w + 16], 7.0);
    }

    #[test]
    fn the_gate_is_the_product_of_sky_and_star_masks() {
        let (w, h) = (32, 32);
        let m = star_mask(w, h, &[(16.0, 16.0, 4.0)]);
        // Sky covers only the left half.
        let sky: Vec<f32> = (0..w * h)
            .map(|i| if i % w < 16 { 1.0 } else { 0.0 })
            .collect();
        let g = build(&m, Some(&sky), 0.0).expect("gate");
        assert!(
            g.allowed_at(15, 16) > 0.0,
            "star inside the sky must be gated open"
        );
        assert_eq!(
            g.allowed_at(17, 16),
            0.0,
            "star outside the sky must be closed"
        );
    }

    #[test]
    fn a_zero_in_the_sky_mask_closes_the_gate_exactly() {
        let (w, h) = (32, 32);
        let m = star_mask(w, h, &[(16.0, 16.0, 4.0)]);
        let sky = vec![0.0f32; w * h];
        let g = build(&m, Some(&sky), 2.0).expect("gate");
        assert!(
            g.allowed.iter().all(|v| *v == 0.0),
            "an all-foreground sky mask must close the gate everywhere"
        );
        let input = ramp(w * h);
        let out = g.composite(&input, &vec![42.0; w * h]).expect("composite");
        assert_eq!(out, input, "nothing may change when the sky mask is closed");
    }

    #[test]
    fn dilation_widens_the_gate() {
        let (w, h) = (64, 64);
        let m = star_mask(w, h, &[(32.0, 32.0, 3.2)]);
        let tight = build(&m, None, 0.0).expect("gate");
        let wide = build(&m, None, 4.0).expect("gate");
        let open = |g: &Gate| g.allowed.iter().filter(|v| **v > 0.0).count();
        assert!(
            open(&wide) > open(&tight),
            "dilation did not widen the gate"
        );
    }

    /// Dilating by a disc, not a square: the corners must not reach further than
    /// the axes.
    #[test]
    fn dilation_uses_a_disc_not_a_square() {
        let (w, h) = (64, 64);
        let m = star_mask(w, h, &[(32.0, 32.0, 2.0)]);
        let g = build(&m, None, 6.0).expect("gate");
        let axis = (0..32)
            .take_while(|&d| g.allowed_at(32 + d, 32) > 0.0)
            .count();
        let diag = (0..32)
            .take_while(|&d| g.allowed_at(32 + d, 32 + d) > 0.0)
            .count();
        assert!(
            diag <= axis,
            "gate reaches further diagonally ({diag}) than axially ({axis}) — square dilation"
        );
    }

    #[test]
    fn a_sky_mask_of_the_wrong_size_is_rejected_not_resampled() {
        let m = star_mask(32, 32, &[(16.0, 16.0, 3.0)]);
        let wrong = vec![1.0f32; 16 * 16];
        assert!(matches!(
            build(&m, Some(&wrong), 0.0).expect_err("must fail"),
            Error::PlaneSize { .. }
        ));
    }

    #[test]
    fn composite_rejects_a_mismatched_plane() {
        let m = star_mask(16, 16, &[(8.0, 8.0, 3.0)]);
        let g = build(&m, None, 0.0).expect("gate");
        assert!(matches!(
            g.composite(&[0.0; 10], &[0.0; 256]).expect_err("must fail"),
            Error::PlaneSize { .. }
        ));
    }

    #[test]
    fn compositing_is_deterministic() {
        let (w, h) = (48, 48);
        let m = star_mask(w, h, &[(24.0, 24.0, 3.2), (10.0, 40.0, 2.5)]);
        let g = build(&m, None, 2.0).expect("gate");
        let input = ramp(w * h);
        let modified: Vec<f32> = input.iter().map(|v| v * 0.5).collect();
        assert_eq!(
            g.composite(&input, &modified).expect("a"),
            g.composite(&input, &modified).expect("b")
        );
    }

    // -- restrict_to_sky ----------------------------------------------------

    fn det(x: f64, y: f64) -> Detection {
        Detection {
            x,
            y,
            flux: 1.0,
            peak: 1.0,
            fwhm: 3.2,
            ellipticity: 0.0,
            theta: 0.0,
            saturated: false,
            tier: Tier::Medium,
            snr: 10.0,
        }
    }

    #[test]
    fn a_detection_well_inside_the_sky_is_kept() {
        let sky = vec![1.0f32; 64 * 64];
        let out = restrict_to_sky(&[det(32.0, 32.0)], &sky, 64, 64, 3.0).expect("restrict");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_detection_on_the_foreground_is_dropped() {
        let mut sky = vec![1.0f32; 64 * 64];
        for y in 40..64 {
            for x in 0..64 {
                sky[y * 64 + x] = 0.0;
            }
        }
        let out = restrict_to_sky(&[det(32.0, 50.0)], &sky, 64, 64, 3.0).expect("restrict");
        assert!(out.is_empty(), "a detection on the foreground survived");
    }

    /// The case a centre test would miss, and the reason the margin exists: the
    /// silhouette's filter response peaks on sky pixels a couple of px out.
    #[test]
    fn a_detection_in_the_sky_but_hugging_the_foreground_is_dropped() {
        let mut sky = vec![1.0f32; 64 * 64];
        for y in 40..64 {
            for x in 0..64 {
                sky[y * 64 + x] = 0.0;
            }
        }
        // y = 38 is sky, but only 2 px from the boundary.
        let out = restrict_to_sky(&[det(32.0, 38.0)], &sky, 64, 64, 3.0).expect("restrict");
        assert!(
            out.is_empty(),
            "a detection hugging the silhouette survived"
        );
        // With no margin it would be kept — which is exactly the bug.
        let out0 = restrict_to_sky(&[det(32.0, 38.0)], &sky, 64, 64, 0.0).expect("restrict");
        assert_eq!(
            out0.len(),
            1,
            "test is vacuous: the margin is doing nothing"
        );
    }

    #[test]
    fn restrict_preserves_order_and_is_deterministic() {
        let sky = vec![1.0f32; 64 * 64];
        let d = vec![det(10.0, 10.0), det(20.0, 20.0), det(30.0, 30.0)];
        let a = restrict_to_sky(&d, &sky, 64, 64, 2.0).expect("a");
        assert_eq!(a, d);
    }
}
