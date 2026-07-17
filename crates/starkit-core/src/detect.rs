//! Star detection and PSF measurement (task T1-3).
//!
//! # Pipeline
//!
//! 1. **Background** ([`crate::background`]) → level and noise maps.
//! 2. **Matched filter.** The background-subtracted plane is correlated with a
//!    normalised Gaussian of the expected width. This is the SNR-optimal way to
//!    find point sources: it raises a star's peak above the noise by ~2.4× at the
//!    fixture's seeing, which is the whole reason faint stars are findable at all.
//!    Separable, so it costs 2·k rather than k² per pixel.
//! 3. **Threshold + local maxima** on the filtered plane. Two stars closer than
//!    the resolution limit merge into one maximum — which is correct, not a bug:
//!    they are one object in this image.
//! 4. **Deblend.** Maxima that share a filtered ridge with no real saddle between
//!    them are one source seen twice; see [`DetectParams::deblend_contrast`].
//! 5. **Measure** each surviving maximum: centroid, FWHM, ellipticity, flux,
//!    saturation.
//! 6. **Tier** by flux tertiles.
//!
//! # Why FWHM is measured from the half-maximum area
//!
//! The obvious estimators are both biased on this PSF family, and T0-2 measured
//! by how much:
//!
//! - **Second moments: +36 %.** A Moffat's second moment is dominated by its
//!   wings and depends on β, which varies per star. Moments measure the wings,
//!   not the half-max width.
//! - **Gaussian fit: +14 % on Moffat stars** (model mismatch), +2 % on Gaussian
//!   ones. The suites are ~60 % Moffat, so the mixture lands near the 10 % bar
//!   with nothing to spare.
//!
//! Instead: the region enclosed by the half-maximum contour of an elliptical
//! profile has area `A = π·a·b/4`, where a and b are the two full widths at half
//! maximum. The catalog defines `fwhm` as the geometric mean `√(a·b)`
//! (docs/FIXTURES.md), so
//!
//! ```text
//!     fwhm = 2·√(A/π)
//! ```
//!
//! — which is the *definition* of FWHM, evaluated directly, and is therefore
//! free of any assumption about profile shape. Moffat, Gaussian or anything else
//! monotonic all measure correctly. `A` is integrated with sub-pixel row
//! crossings rather than by counting pixels: at a 3.2 px FWHM the contour encloses
//! only ~8 pixels, so integer counting alone would quantise FWHM by ~6 %.
//!
//! Ellipticity and orientation still come from second moments: their *magnitude*
//! is biased, but the axis *ratio* and angle are not — the bias scales both axes
//! together and cancels.
//!
//! # Determinism (INV-2)
//!
//! Single-threaded; no map iteration reaches an output; detections are emitted in
//! raster order of their peak pixel, then given ids in that order.

use crate::background::{self, Background, BackgroundParams};
use crate::types::Tier;
use crate::Error;

/// Detection parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectParams {
    /// Threshold on the matched-filtered plane, in units of the filtered noise.
    pub nsigma: f64,
    /// Expected FWHM in pixels — the matched filter's width.
    pub fwhm_guess: f64,
    /// A maximum is discarded as a duplicate of a brighter neighbour if the
    /// filtered valley between them never drops below this fraction of the
    /// fainter peak. 1.0 disables deblending (everything merges); 0.0 keeps every
    /// local maximum.
    pub deblend_contrast: f64,
    /// Level at or above which a pixel is saturated, in the plane's own units.
    pub saturation: f32,
    pub background: BackgroundParams,
}

impl Default for DetectParams {
    fn default() -> Self {
        Self {
            // 5σ on the *filtered* plane. The matched filter multiplies a star's
            // SNR by ~2.4, so this is far below the SNR≥5 population, while the
            // ~1.4M independent resolution elements in a 4096² frame yield well
            // under one 5σ noise peak by chance.
            nsigma: 5.0,
            fwhm_guess: 3.2,
            deblend_contrast: 0.5,
            saturation: 1.0,
            background: BackgroundParams::default(),
        }
    }
}

/// One detected star, before it becomes a catalog entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub x: f64,
    pub y: f64,
    pub flux: f64,
    pub peak: f64,
    pub fwhm: f64,
    pub ellipticity: f64,
    pub theta: f64,
    pub saturated: bool,
    pub tier: Tier,
    /// Peak SNR in the plane as measured — `peak / local noise`.
    pub snr: f64,
}

/// How far the detection filter reaches, in pixels.
///
/// Exposed because [`crate::gate::restrict_to_sky`] must exclude a band at least
/// this wide around a silhouette: a pixel whose kernel footprint touches the
/// foreground has its contrast measured against it, which is what produces
/// phantom stars along every branch (D-019/D-034). Deriving the two from one
/// function is what stops the mask margin and the kernel from drifting apart —
/// a margin quietly smaller than the reach looks like it works and does not.
pub fn filter_reach_px(fwhm_guess: f64) -> f64 {
    let sigma = fwhm_guess / 2.354_820_045_030_949_4;
    (3.0 * sigma).ceil().max(1.0)
}

/// A normalised 1-D Gaussian kernel, truncated at 3σ.
fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (3.0 * sigma).ceil().max(1.0) as usize;
    let mut k: Vec<f64> = (0..=2 * radius)
        .map(|i| {
            let d = i as f64 - radius as f64;
            (-0.5 * (d / sigma) * (d / sigma)).exp()
        })
        .collect();
    let sum: f64 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k
}

/// Separable correlation with edge clamping.
fn convolve_separable(src: &[f32], w: usize, h: usize, k: &[f64]) -> Vec<f32> {
    let r = (k.len() / 2) as isize;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                acc += src[y * w + xx] as f64 * kv;
            }
            tmp[y * w + x] = acc as f32;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f64;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                acc += tmp[yy * w + x] as f64 * kv;
            }
            out[y * w + x] = acc as f32;
        }
    }
    out
}

/// Sub-pixels per pixel edge when integrating the half-maximum area.
///
/// The area must be sub-pixel accurate in **both** axes. An earlier version
/// interpolated the contour crossings along each row but counted whole rows, and
/// the residual quantisation in y made the measured FWHM oscillate with the true
/// one (-13.6 % at 1.8 px, +6.5 % at 2.5 px, +2.7 % at 3.2 px) — a bias that
/// changes sign is a tell that the estimator, not the profile, is at fault.
///
/// At ×8 a 3.2 px FWHM contour is traced by ~80 boundary sub-cells of area
/// 1/64 each, so the quantisation left in the area is a few tenths of a percent
/// — well under a percent of FWHM.
const AREA_SUPERSAMPLE: usize = 8;

/// Bilinear sample of a plane at continuous coordinates, clamped at the edges.
fn bilinear(sub: &[f32], w: usize, h: usize, x: f64, y: f64) -> f64 {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let top = sub[y0 * w + x0] as f64 * (1.0 - fx) + sub[y0 * w + x1] as f64 * fx;
    let bot = sub[y1 * w + x0] as f64 * (1.0 - fx) + sub[y1 * w + x1] as f64 * fx;
    top * (1.0 - fy) + bot * fy
}

/// Area enclosed by the `half` contour containing `(px, py)`.
///
/// Flood-fills a ×[`AREA_SUPERSAMPLE`] bilinear resampling of the neighbourhood,
/// starting at the peak and accepting sub-cells above `half`. The flood fill is
/// what keeps a neighbouring star's contour from being annexed: only the region
/// *connected to this peak* is counted, so a companion 2 px away contributes
/// nothing unless the two genuinely share an isophote — in which case they are
/// one object at this level, which is the truth about the image.
fn half_max_area(
    sub: &[f32],
    w: usize,
    h: usize,
    px: usize,
    py: usize,
    half: f64,
    max_radius: usize,
) -> f64 {
    let s = AREA_SUPERSAMPLE;
    let r = max_radius.max(1);
    let n = 2 * r * s + 1; // sub-grid is (2r+1) pixels wide at s samples per pixel
    let step = 1.0 / s as f64;
    let origin_x = px as f64 - r as f64;
    let origin_y = py as f64 - r as f64;

    let above = |i: usize, j: usize| -> bool {
        let x = origin_x + i as f64 * step;
        let y = origin_y + j as f64 * step;
        bilinear(sub, w, h, x, y) >= half
    };

    let centre = (r * s, r * s);
    if !above(centre.0, centre.1) {
        return 0.0;
    }

    // Iterative flood fill: a recursive one would blow the stack on a saturated
    // star, whose half-max region can span thousands of sub-cells.
    let mut seen = vec![false; n * n];
    let mut stack = vec![centre];
    seen[centre.1 * n + centre.0] = true;
    let mut count = 0usize;

    while let Some((i, j)) = stack.pop() {
        count += 1;
        // 4-connected: an 8-connected fill can leak diagonally across a
        // one-sub-cell saddle between two touching stars.
        let neighbours = [
            (i.wrapping_sub(1), j),
            (i + 1, j),
            (i, j.wrapping_sub(1)),
            (i, j + 1),
        ];
        for (ni, nj) in neighbours {
            if ni >= n || nj >= n || seen[nj * n + ni] {
                continue;
            }
            seen[nj * n + ni] = true;
            if above(ni, nj) {
                stack.push((ni, nj));
            }
        }
    }

    count as f64 * step * step
}

/// Flux-weighted centroid and second moments over a window, using only pixels
/// above `floor` so the noise outside the star contributes nothing.
struct Moments {
    x: f64,
    y: f64,
    ellipticity: f64,
    theta: f64,
}

fn moments(sub: &[f32], w: usize, h: usize, px: usize, py: usize, r: usize, floor: f64) -> Moments {
    let x0 = px.saturating_sub(r);
    let x1 = (px + r + 1).min(w);
    let y0 = py.saturating_sub(r);
    let y1 = (py + r + 1).min(h);

    let (mut s, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let v = sub[y * w + x] as f64;
            if v <= floor {
                continue;
            }
            s += v;
            sx += v * x as f64;
            sy += v * y as f64;
        }
    }
    if s <= 0.0 {
        return Moments {
            x: px as f64,
            y: py as f64,
            ellipticity: 0.0,
            theta: 0.0,
        };
    }
    let (cx, cy) = (sx / s, sy / s);

    let (mut mxx, mut myy, mut mxy) = (0.0f64, 0.0f64, 0.0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let v = sub[y * w + x] as f64;
            if v <= floor {
                continue;
            }
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            mxx += v * dx * dx;
            myy += v * dy * dy;
            mxy += v * dx * dy;
        }
    }
    mxx /= s;
    myy /= s;
    mxy /= s;

    // Eigenvalues of the 2x2 moment matrix give the principal axes.
    let common = (((mxx - myy) * (mxx - myy)) / 4.0 + mxy * mxy).sqrt();
    let lam1 = (mxx + myy) / 2.0 + common; // major
    let lam2 = (mxx + myy) / 2.0 - common; // minor
    let ellipticity = if lam1 > 0.0 && lam2 >= 0.0 {
        (1.0 - (lam2 / lam1).sqrt()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Truth reports theta in [0, pi): an ellipse is symmetric under a half turn.
    let theta = (0.5 * (2.0 * mxy).atan2(mxx - myy)).rem_euclid(std::f64::consts::PI);

    Moments {
        x: cx,
        y: cy,
        ellipticity,
        theta,
    }
}

/// Detect stars in a single linear plane.
///
/// `plane` is row-major, `width * height`, linear light (INV-4).
pub fn detect(
    plane: &[f32],
    width: usize,
    height: usize,
    params: &DetectParams,
) -> Result<Vec<Detection>, Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyImage);
    }
    if plane.len() != width * height {
        return Err(Error::PlaneSize {
            expected: width * height,
            actual: plane.len(),
        });
    }
    if !(params.fwhm_guess.is_finite() && params.fwhm_guess > 0.0) {
        return Err(Error::InvalidParam(
            "fwhm_guess must be finite and positive",
        ));
    }

    let bg: Background = background::estimate(plane, width, height, &params.background)?;
    let sub: Vec<f32> = plane
        .iter()
        .zip(&bg.level)
        .map(|(v, l)| if v.is_finite() { v - l } else { 0.0 })
        .collect();

    let sigma_g = params.fwhm_guess / 2.354_820_045_030_949_4;
    let kernel = gaussian_kernel(sigma_g);
    let support = kernel.len();
    let n = (support * support) as f64;

    // The filter is the Gaussian with its own mean removed, so the kernel sums to
    // zero: `K = g - 1/n`. It therefore measures how much a pixel exceeds its
    // *neighbourhood*, not its absolute level.
    //
    // This matters wherever a star sits on something bright. A plain sum-1
    // Gaussian passes the local level through, so the threshold — which is set
    // from the *sky* noise — is met by ordinary shot noise riding on a bright
    // halo. Measured on the `saturated` suite, that cost 415 false positives
    // against 500 real stars (precision 55 %), sitting a median of 22 px out in
    // the halos, where the local noise is ~26 against the sky's ~11. A zero-sum
    // kernel sees no contrast there and stays quiet.
    //
    // K is not separable, but `conv(img, g) - boxmean(img)` is, and is identical.
    let box_kernel = vec![1.0 / support as f64; support];
    let gauss = convolve_separable(&sub, width, height, &kernel);
    let boxmean = convolve_separable(&sub, width, height, &box_kernel);
    let filtered: Vec<f32> = gauss.iter().zip(&boxmean).map(|(g, b)| g - b).collect();

    // sigma_K = sigma * sqrt(sum(K^2)) = sigma * sqrt(sum(g^2) - 1/n).
    let k2: f64 = kernel.iter().map(|v| v * v).sum();
    let g2 = k2 * k2; // sum over the separable 2-D Gaussian
    let noise_gain = (g2 - 1.0 / n).max(1e-12).sqrt();

    // --- threshold + local maxima -------------------------------------------
    let mut peaks: Vec<(usize, usize, f64)> = Vec::new();
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let i = y * width + x;
            let v = filtered[i] as f64;
            let thr = params.nsigma * bg.rms[i] as f64 * noise_gain;
            if v < thr {
                continue;
            }
            // Strict against earlier neighbours, non-strict against later ones:
            // a flat-topped plateau yields exactly one maximum instead of none.
            let mut is_max = true;
            for dy in 0..3usize {
                for dx in 0..3usize {
                    if dx == 1 && dy == 1 {
                        continue;
                    }
                    let nx = x + dx - 1;
                    let ny = y + dy - 1;
                    let nv = filtered[ny * width + nx] as f64;
                    let earlier = ny < y || (ny == y && nx < x);
                    if (earlier && nv >= v) || (!earlier && nv > v) {
                        is_max = false;
                    }
                }
            }
            if is_max {
                peaks.push((x, y, v));
            }
        }
    }

    // --- deblend -------------------------------------------------------------
    let peaks = deblend(&sub, width, height, peaks, params);

    // --- measure -------------------------------------------------------------
    // Window for the half-max flood fill and the flux aperture: generous, because
    // both are bounded by their own criteria rather than by this radius.
    let r = (2.5 * params.fwhm_guess).ceil() as usize;
    // Window for the centroid and second moments: deliberately tight.
    //
    // A wide window is not "more information", it is more of the *neighbour's*
    // light. At 2.5x FWHM the window is 8 px, and basic-5k places stars a minimum
    // of 8 px apart — so a neighbour sat exactly on the rim and dragged the
    // centroid past the 1.6 px match radius. That single mistake cost recall and
    // precision at once: the star went unmatched (recall 96.5 %) and its own
    // displaced detection was then counted as a false positive (precision 95.8 %),
    // which is why a star at SNR 202 could appear "undetected".
    let r_cent = (1.2 * params.fwhm_guess).ceil() as usize;
    let mut out = Vec::with_capacity(peaks.len());
    for (px, py, _) in peaks {
        let i = py * width + px;
        let noise = bg.rms[i] as f64;
        let m = moments(&sub, width, height, px, py, r_cent, noise);

        // The peak of the *unfiltered* star: the filter is for finding, not for
        // measuring — it deliberately broadens what it detects.
        let mut peak = 0.0f64;
        for y in py.saturating_sub(1)..(py + 2).min(height) {
            for x in px.saturating_sub(1)..(px + 2).min(width) {
                peak = peak.max(sub[y * width + x] as f64);
            }
        }
        if peak <= 0.0 {
            continue;
        }

        let area = half_max_area(&sub, width, height, px, py, peak / 2.0, r);
        let fwhm = 2.0 * (area / std::f64::consts::PI).sqrt();

        // Flux inside the aperture; the wings beyond it are not measured.
        let mut flux = 0.0f64;
        let ap = (1.5 * params.fwhm_guess).max(2.0);
        let ap2 = ap * ap;
        for y in py.saturating_sub(r)..(py + r + 1).min(height) {
            for x in px.saturating_sub(r)..(px + r + 1).min(width) {
                let dx = x as f64 - m.x;
                let dy = y as f64 - m.y;
                if dx * dx + dy * dy <= ap2 {
                    flux += sub[y * width + x] as f64;
                }
            }
        }

        let saturated = (px.saturating_sub(1)..(px + 2).min(width))
            .flat_map(|x| (py.saturating_sub(1)..(py + 2).min(height)).map(move |y| (x, y)))
            .any(|(x, y)| plane[y * width + x] >= params.saturation);

        out.push(Detection {
            x: m.x,
            y: m.y,
            flux,
            peak,
            fwhm,
            ellipticity: m.ellipticity,
            theta: m.theta,
            saturated,
            tier: Tier::Small, // replaced below, once every flux is known
            snr: if noise > 0.0 { peak / noise } else { 0.0 },
        });
    }

    assign_tiers(&mut out);
    Ok(out)
}

/// Drop maxima that are a brighter star's second peak.
///
/// Two maxima belong to one source when the valley between them never falls below
/// `deblend_contrast × the fainter peak`: a real pair has a genuine dip, while
/// noise on one star's shoulder does not. Only pairs within a few FWHM are
/// considered — further apart, a shared ridge is not plausible.
///
/// The saddle is judged on the **unfiltered** plane, not the matched-filtered
/// one. The filter exists to make faint peaks findable, and it does that by
/// smoothing — which fills in the very valley this test needs to see. Measured on
/// a genuine pair 2×FWHM apart: the filtered valley sits at ~0.50 of the peak,
/// indistinguishable from a noise shoulder, while the unfiltered valley is at
/// ~0.12. Find on the filtered plane; decide on the real one.
fn deblend(
    sub: &[f32],
    width: usize,
    height: usize,
    mut peaks: Vec<(usize, usize, f64)>,
    params: &DetectParams,
) -> Vec<(usize, usize, f64)> {
    if params.deblend_contrast <= 0.0 || peaks.len() < 2 {
        return peaks;
    }
    // Brightest first, so a faint shoulder is judged against its parent and never
    // the other way round. Ties broken by position to stay deterministic.
    peaks.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));

    let reach = (3.0 * params.fwhm_guess).ceil();
    let mut keep = vec![true; peaks.len()];
    for i in 0..peaks.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..peaks.len() {
            if !keep[j] {
                continue;
            }
            let (dx, dy) = (
                peaks[j].0 as f64 - peaks[i].0 as f64,
                peaks[j].1 as f64 - peaks[i].1 as f64,
            );
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > reach || dist == 0.0 {
                continue;
            }
            // Walk the segment between the two peaks and find the valley.
            let steps = dist.ceil() as usize * 2;
            let mut valley = f64::INFINITY;
            for s in 1..steps {
                let t = s as f64 / steps as f64;
                let x = (peaks[i].0 as f64 + dx * t).round() as usize;
                let y = (peaks[i].1 as f64 + dy * t).round() as usize;
                if x >= width || y >= height {
                    continue;
                }
                valley = valley.min(sub[y * width + x] as f64);
            }
            let peak_j = sub[peaks[j].1 * width + peaks[j].0] as f64;
            if valley.is_finite() && peak_j > 0.0 && valley > params.deblend_contrast * peak_j {
                // No real dip: one source, seen twice.
                keep[j] = false;
            }
        }
    }

    let mut out: Vec<(usize, usize, f64)> = peaks
        .into_iter()
        .zip(&keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect();
    // Raster order, so ids do not depend on brightness ties (INV-2).
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

/// Flux tertiles → tiers, matching how the truth catalog tiers its population.
fn assign_tiers(stars: &mut [Detection]) {
    if stars.is_empty() {
        return;
    }
    let mut sorted: Vec<f64> = stars.iter().map(|s| s.flux).collect();
    sorted.sort_by(f64::total_cmp);
    let lo = sorted[sorted.len() / 3];
    let hi = sorted[(sorted.len() * 2) / 3];
    for s in stars.iter_mut() {
        s.tier = if s.flux >= hi {
            Tier::Large
        } else if s.flux >= lo {
            Tier::Medium
        } else {
            Tier::Small
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a Gaussian star into a plane, integrating over each pixel by
    /// supersampling — the same convention the fixtures use, so a test star and a
    /// fixture star mean the same thing.
    fn add_star(plane: &mut [f32], w: usize, h: usize, cx: f64, cy: f64, peak: f64, fwhm: f64) {
        let sigma = fwhm / 2.354_820_045_030_949_4;
        let r = (4.0 * fwhm).ceil() as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (cx.round() as i64 + dx, cy.round() as i64 + dy);
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let mut acc = 0.0f64;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = x as f64 - 0.5 + (sx as f64 + 0.5) / 4.0;
                        let py = y as f64 - 0.5 + (sy as f64 + 0.5) / 4.0;
                        let d2 = (px - cx).powi(2) + (py - cy).powi(2);
                        acc += (-d2 / (2.0 * sigma * sigma)).exp();
                    }
                }
                plane[y as usize * w + x as usize] += (peak * acc / 16.0) as f32;
            }
        }
    }

    fn plane_with(stars: &[(f64, f64, f64, f64)], w: usize, h: usize, level: f64) -> Vec<f32> {
        let mut p = vec![level as f32; w * h];
        for &(x, y, peak, fwhm) in stars {
            add_star(&mut p, w, h, x, y, peak, fwhm);
        }
        p
    }

    fn params() -> DetectParams {
        DetectParams {
            saturation: 60000.0,
            ..Default::default()
        }
    }

    #[test]
    fn finds_a_single_star_at_its_true_position() {
        let (w, h) = (128, 128);
        let p = plane_with(&[(64.3, 70.7, 2000.0, 3.2)], w, h, 100.0);
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 1, "expected exactly one star, got {}", d.len());
        // Tolerance: well inside FR-2's 0.3 px bar; a noiseless star should be
        // far better than the requirement.
        assert!((d[0].x - 64.3).abs() < 0.05, "x = {}", d[0].x);
        assert!((d[0].y - 70.7).abs() < 0.05, "y = {}", d[0].y);
    }

    /// The estimator's reason for existing: FWHM from the half-max area is
    /// shape-independent, so on a noiseless well-sampled star it should be
    /// essentially exact rather than merely close.
    ///
    /// **Tolerance 2 %.** Measured residual across this range is ≤ 0.5 %; 2 % is
    /// tight enough that reintroducing either of the biases this estimator exists
    /// to avoid (moments +36 %, Gaussian fit +14 % on Moffat) fails instantly.
    #[test]
    fn measures_fwhm_of_a_gaussian_star() {
        for fwhm in [2.5, 3.0, 3.2, 3.7, 4.5, 6.0, 8.0] {
            let (w, h) = (128, 128);
            let p = plane_with(&[(64.0, 64.0, 3000.0, fwhm)], w, h, 100.0);
            let d = detect(&p, w, h, &params()).expect("detect");
            assert_eq!(d.len(), 1);
            let err = (d[0].fwhm - fwhm).abs() / fwhm;
            assert!(
                err < 0.02,
                "fwhm {fwhm}: measured {} ({:+.1} %)",
                d[0].fwhm,
                (d[0].fwhm / fwhm - 1.0) * 100.0
            );
        }
    }

    /// Below Nyquist the width is not in the pixels to be recovered, and the
    /// estimator reads low (-7 % at 1.8 px). Pinned as a known limit rather than
    /// left to be rediscovered: the suites put the FWHM floor at 1.6 px and the
    /// mean at 3.2, so almost nothing lands here.
    #[test]
    fn undersampled_stars_read_low_and_that_is_a_known_limit() {
        let (w, h) = (128, 128);
        let p = plane_with(&[(64.0, 64.0, 3000.0, 1.8)], w, h, 100.0);
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 1);
        let err = (d[0].fwhm / 1.8 - 1.0) * 100.0;
        assert!(
            (-12.0..0.0).contains(&err),
            "undersampled bias moved from the measured -7 %: now {err:+.1} %"
        );
    }

    #[test]
    fn finds_every_star_in_a_sparse_field() {
        let (w, h) = (256, 256);
        let stars: Vec<(f64, f64, f64, f64)> = (0..20)
            .map(|i| {
                let x = 20.0 + (i % 5) as f64 * 50.0 + 0.37;
                let y = 20.0 + (i / 5) as f64 * 50.0 + 0.61;
                (x, y, 500.0 + i as f64 * 100.0, 3.2)
            })
            .collect();
        let p = plane_with(&stars, w, h, 100.0);
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 20, "expected 20 stars, found {}", d.len());
    }

    #[test]
    fn resolves_a_pair_two_fwhm_apart() {
        let (w, h) = (128, 128);
        let p = plane_with(
            &[(60.0, 64.0, 2000.0, 3.2), (66.4, 64.0, 2000.0, 3.2)],
            w,
            h,
            100.0,
        );
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 2, "a 2x FWHM pair must resolve, got {}", d.len());
    }

    /// A pair far below the resolution limit is *one* object in this image.
    /// Reporting two would be inventing information the pixels do not contain.
    #[test]
    fn a_pair_half_a_fwhm_apart_is_correctly_seen_as_one() {
        let (w, h) = (128, 128);
        let p = plane_with(
            &[(64.0, 64.0, 2000.0, 3.2), (65.6, 64.0, 2000.0, 3.2)],
            w,
            h,
            100.0,
        );
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 1, "an unresolvable pair must not be split");
    }

    #[test]
    fn flags_a_saturated_star() {
        let (w, h) = (128, 128);
        let mut p = plane_with(&[(64.0, 64.0, 30000.0, 3.2)], w, h, 100.0);
        for y in 62..67 {
            for x in 62..67 {
                p[y * w + x] = 65535.0;
            }
        }
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 1);
        assert!(d[0].saturated, "a clipped core must be flagged");
    }

    #[test]
    fn does_not_flag_an_unsaturated_star() {
        let (w, h) = (128, 128);
        let p = plane_with(&[(64.0, 64.0, 2000.0, 3.2)], w, h, 100.0);
        let d = detect(&p, w, h, &params()).expect("detect");
        assert!(!d[0].saturated);
    }

    #[test]
    fn an_empty_sky_yields_no_detections() {
        let p = vec![100.0f32; 128 * 128];
        assert!(detect(&p, 128, 128, &params()).expect("detect").is_empty());
    }

    #[test]
    fn is_deterministic_across_runs() {
        let (w, h) = (192, 192);
        let stars: Vec<(f64, f64, f64, f64)> = (0..12)
            .map(|i| {
                (
                    15.0 + (i % 4) as f64 * 45.0,
                    15.0 + (i / 4) as f64 * 45.0,
                    800.0,
                    3.2,
                )
            })
            .collect();
        let p = plane_with(&stars, w, h, 100.0);
        let a = detect(&p, w, h, &params()).expect("a");
        let b = detect(&p, w, h, &params()).expect("b");
        assert_eq!(a, b, "detection is not deterministic");
    }

    #[test]
    fn measures_ellipticity_of_an_elongated_source() {
        // A crude 2:1 elongated blob: two overlapping stars side by side.
        let (w, h) = (128, 128);
        let p = plane_with(
            &[(63.0, 64.0, 2000.0, 3.2), (65.0, 64.0, 2000.0, 3.2)],
            w,
            h,
            100.0,
        );
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 1);
        assert!(
            d[0].ellipticity > 0.1,
            "elongation not seen: e = {}",
            d[0].ellipticity
        );
        // Elongated along x ⇒ theta near 0 or pi.
        let t = d[0].theta;
        assert!(
            !(0.3..=std::f64::consts::PI - 0.3).contains(&t),
            "theta {t} does not point along x"
        );
    }

    #[test]
    fn tiers_split_the_population_into_thirds() {
        let (w, h) = (256, 256);
        let stars: Vec<(f64, f64, f64, f64)> = (0..30)
            .map(|i| {
                let x = 20.0 + (i % 6) as f64 * 40.0;
                let y = 20.0 + (i / 6) as f64 * 45.0;
                (x, y, 300.0 + i as f64 * 300.0, 3.2)
            })
            .collect();
        let p = plane_with(&stars, w, h, 100.0);
        let d = detect(&p, w, h, &params()).expect("detect");
        assert_eq!(d.len(), 30);
        let count = |t: Tier| d.iter().filter(|s| s.tier == t).count();
        assert_eq!(count(Tier::Small), 10);
        assert_eq!(count(Tier::Medium), 10);
        assert_eq!(count(Tier::Large), 10);
    }

    #[test]
    fn rejects_a_mismatched_plane() {
        assert!(matches!(
            detect(&[0.0; 10], 4, 4, &params()).expect_err("must fail"),
            Error::PlaneSize { .. }
        ));
    }
}
