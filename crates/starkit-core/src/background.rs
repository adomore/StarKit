//! Tiled sigma-clipped background and noise estimation (task T1-2).
//!
//! # Algorithm, stated exactly (starkit-core/CLAUDE.md)
//!
//! 1. **Tile.** The plane is cut into `box_size × box_size` tiles. Edge tiles are
//!    partial and are used as-is rather than padded — padding would invent data.
//! 2. **Sigma-clip each tile.** Iterate: take the median and the standard
//!    deviation of the surviving samples, then reject everything more than
//!    `sigma × std` from the median. Repeat until nothing new is rejected or
//!    `max_iters` is reached. The **median** is the centre, not the mean: a star
//!    field is background plus a heavy positive tail, and a mean would be dragged
//!    up by exactly the pixels we are trying to reject, which is how the first
//!    iteration decides what to keep.
//! 3. **Estimate.** Tile level = median of survivors; tile RMS = standard
//!    deviation of survivors. Both are computed only from what survived, so a
//!    star sitting in the tile contributes nothing once clipped.
//! 4. **Median-filter the tile grid.** A tile can be so dominated by one bright
//!    star that clipping cannot save it. Its neighbours cannot all be, so a
//!    `filter_size × filter_size` median over the low-resolution grid replaces
//!    the outlier with something plausible.
//! 5. **Interpolate.** Bilinearly, from tile centres back to full resolution.
//!
//! # Why the estimate is biased low, and by how much
//!
//! Clipping a symmetric noise distribution at ±σ·k throws away both tails, so the
//! surviving standard deviation *underestimates* the true one. The factor is
//! deterministic and depends only on `sigma`, so it is corrected analytically —
//! see [`clip_std_correction`]. Without it the noise map reads ~11 % low at
//! sigma = 3, and every downstream SNR would be ~11 % optimistic.
//!
//! # Determinism (INV-2)
//!
//! Single-threaded; medians are taken by sorting with `f64::total_cmp`; no map
//! iteration order reaches an output. Tiles are independent and the loop is a
//! plain `map`, so T1-9 can hand it to rayon without touching the maths.

use crate::Error;

/// Estimation parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundParams {
    /// Tile edge in pixels. Small enough to track a gradient, large enough that a
    /// star cannot dominate its tile's median.
    pub box_size: usize,
    /// Edge of the median filter applied to the tile grid. Must be odd; 1
    /// disables it.
    pub filter_size: usize,
    /// Clip threshold, in standard deviations.
    pub sigma: f64,
    /// Cap on clipping iterations. Convergence is usually 2-4.
    pub max_iters: usize,
}

impl Default for BackgroundParams {
    /// 64 px tiles, 3×3 grid median, 3σ clipping.
    ///
    /// 64 px is ~20 × the typical 3.2 px FWHM: a star covers well under 1 % of a
    /// tile's 4096 samples, so it cannot move the median even before clipping,
    /// while the tile is still short enough to follow a sky gradient.
    fn default() -> Self {
        Self {
            box_size: 64,
            filter_size: 3,
            sigma: 3.0,
            max_iters: 10,
        }
    }
}

/// A full-resolution background level and noise map.
#[derive(Debug, Clone)]
pub struct Background {
    pub width: usize,
    pub height: usize,
    /// Estimated background level per pixel.
    pub level: Vec<f32>,
    /// Estimated noise (standard deviation) per pixel.
    pub rms: Vec<f32>,
}

impl Background {
    #[inline]
    pub fn level_at(&self, x: usize, y: usize) -> f32 {
        self.level[y * self.width + x]
    }

    #[inline]
    pub fn rms_at(&self, x: usize, y: usize) -> f32 {
        self.rms[y * self.width + x]
    }
}

/// The factor by which symmetric clipping at ±`sigma` shrinks the standard
/// deviation of a Gaussian.
///
/// For a standard normal truncated to `[-k, k]`, the surviving variance is
///
/// ```text
///   1 - 2·k·φ(k) / (2·Φ(k) - 1)
/// ```
///
/// where φ is the normal pdf and Φ the cdf. Dividing the measured std by the
/// square root of that recovers an unbiased estimate. At k = 3 the factor is
/// ≈ 0.9915 in variance — the correction is small but systematic, and a
/// systematic 1 % error in the noise map is a systematic error in every SNR
/// derived from it.
pub fn clip_std_correction(sigma: f64) -> f64 {
    if !(sigma.is_finite() && sigma > 0.0) {
        return 1.0;
    }
    let phi = (-0.5 * sigma * sigma).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let big_phi = 0.5 * (1.0 + erf(sigma / std::f64::consts::SQRT_2));
    let denom = 2.0 * big_phi - 1.0;
    if denom <= 0.0 {
        return 1.0;
    }
    let var_ratio = 1.0 - 2.0 * sigma * phi / denom;
    if var_ratio <= 0.0 {
        1.0
    } else {
        var_ratio.sqrt()
    }
}

/// Abramowitz & Stegun 7.1.26. Max absolute error 1.5e-7 — far below any
/// precision that matters for a correction factor of order 1.
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Median of a slice, by sorting a copy with `total_cmp`.
///
/// `total_cmp` rather than `partial_cmp().unwrap()`: a NaN in the data would
/// make the latter panic on a data-driven path, which starkit-core forbids.
/// Here NaN simply sorts to one end and the median stays defined.
fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if !n.is_multiple_of(2) {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    }
}

/// One tile's sigma-clipped `(level, rms)`.
///
/// Returns `None` when clipping leaves too little to measure — the caller fills
/// the hole from neighbours rather than inventing a number here.
fn clipped_stats(values: &mut Vec<f64>, sigma: f64, max_iters: usize) -> Option<(f64, f64)> {
    if values.len() < 2 {
        return None;
    }
    let correction = clip_std_correction(sigma);

    for _ in 0..max_iters.max(1) {
        let centre = median(values);
        // f64 accumulator over f32 pixels, per the workspace numerics rule.
        let n = values.len() as f64;
        let var = values
            .iter()
            .map(|v| (v - centre) * (v - centre))
            .sum::<f64>()
            / n;
        let std = var.sqrt();
        if !std.is_finite() || std <= 0.0 {
            // A perfectly flat tile: nothing to clip, and the answer is exact.
            return Some((centre, 0.0));
        }
        let lo = centre - sigma * std;
        let hi = centre + sigma * std;
        let before = values.len();
        values.retain(|v| *v >= lo && *v <= hi);
        if values.len() < 2 {
            return None;
        }
        if values.len() == before {
            break; // converged
        }
    }

    let centre = median(values);
    let n = values.len() as f64;
    let var = values
        .iter()
        .map(|v| (v - centre) * (v - centre))
        .sum::<f64>()
        / n;
    let std = var.sqrt() / correction;
    if centre.is_finite() && std.is_finite() {
        Some((centre, std))
    } else {
        None
    }
}

/// Median filter over the low-resolution tile grid, ignoring holes.
///
/// Out-of-range neighbours are **clamped to the nearest tile**, not skipped.
/// Skipping them looks harmless and is not: at a border the window becomes
/// one-sided, so on a sky gradient the edge tile takes the median of itself and
/// its inward neighbours and is pulled toward the frame centre. Measured on a
/// 100→140 ramp, the first tile read 110.0 instead of 104.9 — a wrong background
/// along all four edges of every gradient image. Clamping duplicates the edge
/// tile instead, which keeps the median on the edge tile's own value.
fn median_filter(grid: &[Option<f64>], nx: usize, ny: usize, size: usize) -> Vec<Option<f64>> {
    if size <= 1 {
        return grid.to_vec();
    }
    let r = (size / 2) as isize;
    let mut out = Vec::with_capacity(grid.len());
    let mut buf = Vec::with_capacity(size * size);
    for j in 0..ny as isize {
        for i in 0..nx as isize {
            buf.clear();
            for dj in -r..=r {
                for di in -r..=r {
                    let x = (i + di).clamp(0, nx as isize - 1) as usize;
                    let y = (j + dj).clamp(0, ny as isize - 1) as usize;
                    if let Some(v) = grid[y * nx + x] {
                        buf.push(v);
                    }
                }
            }
            out.push(if buf.is_empty() {
                None
            } else {
                Some(median(&mut buf))
            });
        }
    }
    out
}

/// Replace holes with the median of the whole grid, so interpolation has
/// something defined everywhere. A hole means every sample in a tile *and* its
/// neighbourhood was clipped away — pathological, but a NaN leaking into the
/// background would silently poison every downstream pixel.
fn fill_holes(grid: &mut [Option<f64>]) -> Option<f64> {
    let mut known: Vec<f64> = grid.iter().filter_map(|v| *v).collect();
    if known.is_empty() {
        return None;
    }
    let fallback = median(&mut known);
    for slot in grid.iter_mut() {
        if slot.is_none() {
            *slot = Some(fallback);
        }
    }
    Some(fallback)
}

/// Bilinear interpolation from tile centres to full resolution, with edge
/// clamping (pixels outside the centre grid take the nearest centre's value).
fn interpolate(
    grid: &[Option<f64>],
    nx: usize,
    ny: usize,
    w: usize,
    h: usize,
    box_size: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    let at =
        |i: usize, j: usize| -> f64 { grid[j.min(ny - 1) * nx + i.min(nx - 1)].unwrap_or(0.0) };
    let half = box_size as f64 / 2.0;

    for y in 0..h {
        // Position in tile-centre space: centre of tile j sits at j*box + box/2.
        let gy = ((y as f64 + 0.5) - half) / box_size as f64;
        let gy = gy.clamp(0.0, (ny - 1) as f64);
        let j0 = gy.floor() as usize;
        let j1 = (j0 + 1).min(ny - 1);
        let fy = gy - j0 as f64;

        for x in 0..w {
            let gx = ((x as f64 + 0.5) - half) / box_size as f64;
            let gx = gx.clamp(0.0, (nx - 1) as f64);
            let i0 = gx.floor() as usize;
            let i1 = (i0 + 1).min(nx - 1);
            let fx = gx - i0 as f64;

            let top = at(i0, j0) * (1.0 - fx) + at(i1, j0) * fx;
            let bot = at(i0, j1) * (1.0 - fx) + at(i1, j1) * fx;
            out[y * w + x] = (top * (1.0 - fy) + bot * fy) as f32;
        }
    }
    out
}

/// Estimate the background level and noise of a single linear plane.
///
/// `plane` is row-major, `width * height`, in linear light (INV-4). Which plane
/// it is — one channel, or the mean of three — is the caller's decision; this
/// function has no opinion about colour.
pub fn estimate(
    plane: &[f32],
    width: usize,
    height: usize,
    params: &BackgroundParams,
) -> Result<Background, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    if plane.len() != width * height {
        return Err(Error::PlaneSize {
            expected: width * height,
            actual: plane.len(),
        });
    }
    if params.box_size == 0 {
        return Err(Error::InvalidParam("box_size must be at least 1"));
    }
    if params.filter_size.is_multiple_of(2) {
        return Err(Error::InvalidParam("filter_size must be odd"));
    }
    if !(params.sigma.is_finite() && params.sigma > 0.0) {
        return Err(Error::InvalidParam("sigma must be finite and positive"));
    }

    let box_size = params.box_size;
    let nx = width.div_ceil(box_size);
    let ny = height.div_ceil(box_size);

    let mut level_grid: Vec<Option<f64>> = Vec::with_capacity(nx * ny);
    let mut rms_grid: Vec<Option<f64>> = Vec::with_capacity(nx * ny);
    let mut buf: Vec<f64> = Vec::with_capacity(box_size * box_size);

    for j in 0..ny {
        for i in 0..nx {
            buf.clear();
            let y1 = ((j + 1) * box_size).min(height);
            let x1 = ((i + 1) * box_size).min(width);
            for y in j * box_size..y1 {
                for x in i * box_size..x1 {
                    let v = plane[y * width + x];
                    if v.is_finite() {
                        buf.push(v as f64);
                    }
                }
            }
            match clipped_stats(&mut buf, params.sigma, params.max_iters) {
                Some((l, r)) => {
                    level_grid.push(Some(l));
                    rms_grid.push(Some(r));
                }
                None => {
                    level_grid.push(None);
                    rms_grid.push(None);
                }
            }
        }
    }

    let mut level_grid = median_filter(&level_grid, nx, ny, params.filter_size);
    let mut rms_grid = median_filter(&rms_grid, nx, ny, params.filter_size);
    if fill_holes(&mut level_grid).is_none() {
        return Err(Error::NoBackground);
    }
    fill_holes(&mut rms_grid);

    Ok(Background {
        width,
        height,
        level: interpolate(&level_grid, nx, ny, width, height, box_size),
        rms: interpolate(&rms_grid, nx, ny, width, height, box_size),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic Gaussian noise without pulling in an RNG: a fixed
    /// Box-Muller over a integer-seeded LCG. The values matter less than the
    /// fact that they are the same on every run (INV-2).
    struct Lcg(u64);
    impl Lcg {
        fn next_f64(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
        fn normal(&mut self) -> f64 {
            let u1 = self.next_f64().max(1e-12);
            let u2 = self.next_f64();
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        }
    }

    fn noisy_plane(w: usize, h: usize, level: f64, sigma: f64, seed: u64) -> Vec<f32> {
        let mut r = Lcg(seed);
        (0..w * h)
            .map(|_| (level + sigma * r.normal()) as f32)
            .collect()
    }

    #[test]
    fn recovers_a_flat_level_with_no_noise() {
        let plane = vec![0.25f32; 128 * 128];
        let bg = estimate(&plane, 128, 128, &BackgroundParams::default()).expect("estimate");
        for v in &bg.level {
            assert!((v - 0.25).abs() < 1e-6, "level {v} != 0.25");
        }
        for v in &bg.rms {
            assert_eq!(*v, 0.0, "a constant plane has no noise");
        }
    }

    /// Tolerance: the median of a 64x64 tile has a standard error of
    /// ~1.253·σ/√4096 = 0.0196·σ. Over a 3x3 grid median and bilinear
    /// interpolation the residual is smaller still; 0.1·σ is ~5 standard errors
    /// and leaves no room for a real bias to hide.
    #[test]
    fn recovers_a_noisy_level() {
        let (w, h, level, sigma) = (256, 256, 100.0, 5.0);
        let plane = noisy_plane(w, h, level, sigma, 42);
        let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");
        let mean_level = bg.level.iter().map(|v| *v as f64).sum::<f64>() / bg.level.len() as f64;
        assert!(
            (mean_level - level).abs() < 0.1 * sigma,
            "level {mean_level} is off by more than 0.1 sigma from {level}"
        );
    }

    /// The clip-bias correction is the point of this test: without it the
    /// recovered sigma reads ~4 % low at 3σ clipping on Gaussian noise.
    /// Tolerance: 8 % — the sampling error of a std over 4096 samples is ~1 %,
    /// so anything approaching the uncorrected bias would still fail.
    #[test]
    fn recovers_the_noise_sigma_after_correcting_the_clip_bias() {
        let (w, h, level, sigma) = (256, 256, 100.0, 5.0);
        let plane = noisy_plane(w, h, level, sigma, 7);
        let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");
        let mean_rms = bg.rms.iter().map(|v| *v as f64).sum::<f64>() / bg.rms.len() as f64;
        assert!(
            (mean_rms - sigma).abs() / sigma < 0.08,
            "rms {mean_rms} differs from {sigma} by more than 8 %"
        );
    }

    #[test]
    fn the_clip_correction_is_below_one_and_approaches_it_as_sigma_grows() {
        let c3 = clip_std_correction(3.0);
        assert!(c3 < 1.0 && c3 > 0.9, "3-sigma correction {c3} out of range");
        // Clipping ever further out throws away ever less.
        assert!(clip_std_correction(5.0) > c3);
        assert!(clip_std_correction(10.0) > clip_std_correction(5.0));
        assert!(clip_std_correction(10.0) <= 1.0);
    }

    #[test]
    fn stars_do_not_lift_the_background() {
        let (w, h) = (256, 256);
        let mut plane = noisy_plane(w, h, 100.0, 5.0, 3);
        // 200 bright stars, each far above the noise.
        let mut r = Lcg(99);
        for _ in 0..200 {
            let x = (r.next_f64() * (w - 4) as f64) as usize + 2;
            let y = (r.next_f64() * (h - 4) as f64) as usize + 2;
            for dy in 0..3 {
                for dx in 0..3 {
                    plane[(y + dy - 1) * w + (x + dx - 1)] += 5000.0;
                }
            }
        }
        let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");
        let mean_level = bg.level.iter().map(|v| *v as f64).sum::<f64>() / bg.level.len() as f64;
        // Without clipping, 200 stars x 9 px x 5000 spread over 65536 px would
        // lift the mean by ~137. The median plus clipping must reject them.
        assert!(
            (mean_level - 100.0).abs() < 1.0,
            "stars lifted the background to {mean_level}"
        );
    }

    #[test]
    fn tracks_a_linear_gradient() {
        let (w, h) = (256, 256);
        // 100 at x=0 rising to 140 at x=255.
        let plane: Vec<f32> = (0..w * h)
            .map(|i| 100.0 + 40.0 * ((i % w) as f32 / (w - 1) as f32))
            .collect();
        let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");
        // Tolerance: bilinear interpolation of a linear ramp is exact except
        // within half a tile of the edges, where clamping flattens it.
        for y in (0..h).step_by(37) {
            for x in 32..(w - 32) {
                let want = 100.0 + 40.0 * (x as f32 / (w - 1) as f32);
                let got = bg.level_at(x, y);
                assert!((got - want).abs() < 0.5, "at ({x},{y}): {got} vs {want}");
            }
        }
    }

    #[test]
    fn is_deterministic_across_runs() {
        let plane = noisy_plane(128, 128, 50.0, 3.0, 11);
        let a = estimate(&plane, 128, 128, &BackgroundParams::default()).expect("a");
        let b = estimate(&plane, 128, 128, &BackgroundParams::default()).expect("b");
        assert_eq!(a.level, b.level, "level is not deterministic");
        assert_eq!(a.rms, b.rms, "rms is not deterministic");
    }

    #[test]
    fn handles_an_image_smaller_than_one_tile() {
        let plane = vec![7.0f32; 10 * 10];
        let bg = estimate(&plane, 10, 10, &BackgroundParams::default()).expect("estimate");
        assert_eq!(bg.level.len(), 100);
        assert!((bg.level_at(5, 5) - 7.0).abs() < 1e-6);
    }

    #[test]
    fn handles_dimensions_that_are_not_a_tile_multiple() {
        let plane = vec![2.0f32; 100 * 70];
        let bg = estimate(&plane, 100, 70, &BackgroundParams::default()).expect("estimate");
        assert_eq!(bg.level.len(), 7000);
        for v in &bg.level {
            assert!((v - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_nan_pixel_does_not_poison_the_estimate() {
        let mut plane = noisy_plane(128, 128, 10.0, 1.0, 5);
        plane[64 * 128 + 64] = f32::NAN;
        let bg = estimate(&plane, 128, 128, &BackgroundParams::default()).expect("estimate");
        assert!(
            bg.level.iter().all(|v| v.is_finite()),
            "NaN leaked into the level map"
        );
        assert!(
            bg.rms.iter().all(|v| v.is_finite()),
            "NaN leaked into the rms map"
        );
    }

    #[test]
    fn rejects_a_mismatched_plane_length() {
        let err = estimate(&[0.0; 10], 4, 4, &BackgroundParams::default()).expect_err("must fail");
        assert!(
            matches!(err, Error::PlaneSize { .. }),
            "wrong variant: {err:?}"
        );
    }

    #[test]
    fn rejects_an_even_filter_size() {
        let p = BackgroundParams {
            filter_size: 2,
            ..Default::default()
        };
        assert!(estimate(&[0.0; 16], 4, 4, &p).is_err());
    }

    #[test]
    fn rejects_an_empty_image() {
        assert!(matches!(
            estimate(&[], 0, 0, &BackgroundParams::default()).expect_err("must fail"),
            Error::EmptyImage
        ));
    }

    #[test]
    fn median_is_the_average_of_the_middle_two_when_even() {
        assert_eq!(median(&mut [1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(median(&mut [3.0, 1.0, 4.0, 2.0]), 2.5);
        assert_eq!(median(&mut [5.0, 1.0, 3.0]), 3.0);
    }
}
