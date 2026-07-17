//! Rendering: stars → noiseless signal → Poisson + read noise → 16-bit
//! quantization (FIXTURES.md "Rendering model").
//!
//! Determinism: single-threaded throughout, one seeded `ChaCha20Rng` drawn in a
//! fixed order (foreground → population → per-star nothing → noise scanned
//! y, x, channel). Rendering is deliberately *not* parallelised in T0-1 —
//! correctness of the truth catalog is the deliverable, speed is not, and rayon
//! would put an ordering hazard on the one code path whose bytes are pinned.

use rand_chacha::ChaCha20Rng;
use rand_distr::{Distribution, Normal, Poisson};

use crate::catalog::{Catalog, ImageMeta, Star, Tier, SCHEMA_V1};
use crate::field::{self, StarSpec};
use crate::foreground::{self, Foreground, SKY};
use crate::params::Params;
use crate::psf::{Psf, PsfKind};

pub struct Rendered {
    pub catalog: Catalog,
    /// Interleaved RGB, `width * height * 3`.
    pub image: Vec<u16>,
    /// Present only when the suite has a foreground.
    pub sky_mask: Option<Vec<u8>>,
}

/// A star's rendered stamp: pixel values in electrons over a bounding box.
struct Stamp {
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
    /// Mean-of-channels signal, already normalised to the star's total flux.
    px: Vec<f64>,
    /// Peak of `px` — the true (pre-clip) peak.
    peak_e: f64,
}

/// Integrate a PSF over a pixel box by ×N supersampling, then normalise the whole
/// stamp to `flux_e`.
///
/// Normalising the *rendered* stamp (rather than trusting the analytic integral)
/// is what makes the truth catalog exact: profile truncation at the stamp edge
/// cannot introduce a flux error, because the stamp is the definition of the
/// star. The full stamp is always integrated — including any part that falls off
/// the frame — so an edge star's catalog flux still means "total flux emitted".
fn render_stamp(spec: &StarSpec, psf: &Psf, flux_e: f64, p: &Params) -> Stamp {
    let radius = (p.psf.stamp_radius_fwhm * psf.fwhm).max(4.0);
    let x0 = (spec.x - radius).floor() as i64;
    let x1 = (spec.x + radius).ceil() as i64;
    let y0 = (spec.y - radius).floor() as i64;
    let y1 = (spec.y + radius).ceil() as i64;
    let w = (x1 - x0 + 1) as usize;
    let h = (y1 - y0 + 1) as usize;

    let ss = p.psf.supersample.max(1);
    let ssf = ss as f64;
    let mut px = vec![0.0f64; w * h];
    let mut total = 0.0f64;

    for iy in 0..h {
        for ix in 0..w {
            // Pixel (i, j) spans [i - 0.5, i + 0.5]: (0,0) is the *centre* of the
            // top-left pixel (coordinate convention frozen in FIXTURES.md).
            let px_x = (x0 + ix as i64) as f64;
            let px_y = (y0 + iy as i64) as f64;
            let mut acc = 0.0f64;
            for sy in 0..ss {
                let sub_y = px_y - 0.5 + (sy as f64 + 0.5) / ssf;
                for sx in 0..ss {
                    let sub_x = px_x - 0.5 + (sx as f64 + 0.5) / ssf;
                    acc += psf.value(sub_x - spec.x, sub_y - spec.y);
                }
            }
            // Box-downsample: mean of sub-samples ≈ integral over the pixel area.
            let v = acc / (ssf * ssf);
            px[iy * w + ix] = v;
            total += v;
        }
    }

    let scale = if total > 0.0 { flux_e / total } else { 0.0 };
    let mut peak_e = 0.0f64;
    for v in &mut px {
        *v *= scale;
        if *v > peak_e {
            peak_e = *v;
        }
    }

    Stamp {
        x0,
        y0,
        w,
        h,
        px,
        peak_e,
    }
}

/// Add a stamp into the interleaved signal plane, applying the star's tint.
fn add_stamp(plane: &mut [f32], width: usize, height: usize, stamp: &Stamp, tint: [f64; 3]) {
    for iy in 0..stamp.h {
        let y = stamp.y0 + iy as i64;
        if y < 0 || y >= height as i64 {
            continue;
        }
        for ix in 0..stamp.w {
            let x = stamp.x0 + ix as i64;
            if x < 0 || x >= width as i64 {
                continue;
            }
            let v = stamp.px[iy * stamp.w + ix];
            let base = (y as usize * width + x as usize) * 3;
            for c in 0..3 {
                plane[base + c] += (v * tint[c]) as f32;
            }
        }
    }
}

fn background_at(p: &Params, x: f64, y: f64) -> f64 {
    let fx = if p.image.width > 1 {
        x / (p.image.width - 1) as f64
    } else {
        0.0
    };
    let fy = if p.image.height > 1 {
        y / (p.image.height - 1) as f64
    } else {
        0.0
    };
    // Gradients are expressed as the total change across the frame, centred so
    // `level_e` is the mean rather than the corner value.
    p.background.level_e
        + p.background.gradient_x_e * (fx - 0.5)
        + p.background.gradient_y_e * (fy - 0.5)
}

/// Flux tertiles → tiers (FIXTURES.md "Star population").
fn assign_tiers(fluxes: &[f64]) -> Vec<Tier> {
    let mut sorted: Vec<f64> = fluxes.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return Vec::new();
    }
    let lo = sorted[sorted.len() / 3];
    let hi = sorted[(sorted.len() * 2) / 3];
    fluxes
        .iter()
        .map(|&f| {
            if f >= hi {
                Tier::Large
            } else if f >= lo {
                Tier::Medium
            } else {
                Tier::Small
            }
        })
        .collect()
}

/// Peak SNR: star peak over the noise at that pixel (shot + background + read).
fn peak_snr(peak_e: f64, bkg_e: f64, read_sigma_e: f64) -> f64 {
    let variance = peak_e + bkg_e + read_sigma_e * read_sigma_e;
    if variance <= 0.0 {
        return 0.0;
    }
    peak_e / variance.sqrt()
}

pub fn render(p: &Params) -> Result<Rendered, String> {
    let w = p.image.width as usize;
    let h = p.image.height as usize;
    let mut rng = <ChaCha20Rng as rand::SeedableRng>::seed_from_u64(p.seed);

    // 1. Foreground first: star placement depends on the sky mask.
    let fgnd: Option<Foreground> = foreground::build(p, &mut rng);
    let sky_mask: Option<Vec<u8>> = fgnd.as_ref().map(|f| f.mask.clone());

    // 2. Population.
    let specs = field::sample_stars(p, sky_mask.as_deref(), &mut rng)?;

    // 3. Noiseless star signal.
    let mut plane = vec![0.0f32; w * h * 3];
    let mut stars = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let stamp = render_stamp(spec, &spec.psf, spec.flux_e, p);
        add_stamp(&mut plane, w, h, &stamp, spec.tint);

        let bkg_e = background_at(p, spec.x, spec.y);
        let peak_adu = stamp.peak_e * p.noise.gain_adu_per_e;
        let bkg_adu = bkg_e * p.noise.gain_adu_per_e;
        let saturated = peak_adu + bkg_adu >= p.noise.saturation_adu;

        // Halo + bleed for bright stars: additional signal, deliberately not part
        // of the catalog flux (see `catalog::Star`).
        if let Some(halo) = &p.halo {
            if spec.flux_e >= halo.flux_threshold_e {
                let halo_psf = Psf {
                    kind: PsfKind::Moffat { beta: 2.5 },
                    fwhm: spec.psf.fwhm * halo.fwhm_factor,
                    ellipticity: 0.0,
                    theta: 0.0,
                };
                let halo_stamp = render_stamp(spec, &halo_psf, spec.flux_e * halo.flux_frac, p);
                add_stamp(&mut plane, w, h, &halo_stamp, spec.tint);
            }
            if halo.bleed && saturated {
                add_bleed(
                    &mut plane,
                    w,
                    h,
                    spec,
                    &stamp,
                    p,
                    halo.bleed_width_px,
                    halo.bleed_length_gain,
                );
            }
        }

        stars.push(Star {
            id: i as u32 + 1,
            x: spec.x,
            y: spec.y,
            flux: spec.flux_e * p.noise.gain_adu_per_e,
            peak: peak_adu,
            fwhm: spec.psf.fwhm,
            ellipticity: spec.psf.ellipticity,
            theta: spec.psf.theta,
            saturated,
            tier: Tier::Small, // replaced below, once every flux is known
            snr: Some(peak_snr(stamp.peak_e, bkg_e, p.noise.read_sigma_e)),
        });
    }

    let tiers = assign_tiers(&specs.iter().map(|s| s.flux_e).collect::<Vec<_>>());
    for (star, tier) in stars.iter_mut().zip(tiers) {
        star.tier = tier;
    }

    // 4. Background, foreground, noise, quantization.
    let image = expose(p, &mut rng, &mut plane, fgnd.as_ref(), w, h);

    Ok(Rendered {
        catalog: Catalog {
            schema: SCHEMA_V1.to_string(),
            image: ImageMeta {
                width: p.image.width,
                height: p.image.height,
                bit_depth: 16,
                color_space: "linear-rgb".to_string(),
            },
            stars,
            generator: Some(p.clone()),
        },
        image,
        sky_mask,
    })
}

/// Vertical blooming column above and below a saturated core. A crude CCD
/// analogue: the excess charge over full-well is spread along the column.
#[allow(clippy::too_many_arguments)]
fn add_bleed(
    plane: &mut [f32],
    w: usize,
    h: usize,
    spec: &StarSpec,
    stamp: &Stamp,
    p: &Params,
    width_px: f64,
    gain: f64,
) {
    let full_well_e = p.noise.saturation_adu / p.noise.gain_adu_per_e;
    // Total charge the core cannot hold. Summing the whole overflow — not just
    // the peak's excess — is what makes the column scale with star brightness the
    // way blooming does; keying off the peak alone yields a column shorter than
    // the already-saturated core, i.e. no visible bleed at all.
    let overflow_e: f64 = stamp.px.iter().map(|&v| (v - full_well_e).max(0.0)).sum();
    if overflow_e <= 0.0 {
        return;
    }
    let half_w = (width_px / 2.0).max(0.5);
    // Half-length of a `width_px`-wide column filled at full well by the
    // overflow, scaled by the stylization factor (see `HaloParams`, D-013).
    let len = (gain * overflow_e / (full_well_e * width_px * 2.0)).min(h as f64 / 4.0);
    if len < 1.0 {
        return;
    }
    let cx = spec.x;
    let y_lo = (spec.y - len).floor().max(0.0) as usize;
    let y_hi = (spec.y + len).ceil().min(h as f64 - 1.0) as usize;
    for y in y_lo..=y_hi {
        // Flat-topped: the inner half of the column sits at full well (and so
        // clips), the outer half fades out. A linear taper alone would leave most
        // of the column below the clip point and invisible.
        let taper = (2.0 * (1.0 - ((y as f64 - spec.y).abs() / len).min(1.0))).min(1.0);
        for x in ((cx - half_w).floor().max(0.0) as usize)
            ..=((cx + half_w).ceil().min(w as f64 - 1.0) as usize)
        {
            let base = (y * w + x) * 3;
            for c in 0..3 {
                plane[base + c] += (full_well_e * taper * spec.tint[c]) as f32;
            }
        }
    }
}

/// Add background/foreground, apply Poisson shot noise + Gaussian read noise,
/// then gain-scale, clamp to `[0, 65535]` and quantize.
fn expose(
    p: &Params,
    rng: &mut ChaCha20Rng,
    plane: &mut [f32],
    fgnd: Option<&Foreground>,
    w: usize,
    h: usize,
) -> Vec<u16> {
    let read = Normal::new(0.0f64, p.noise.read_sigma_e)
        .expect("read_sigma_e is a non-negative constant in every suite preset");
    let mut out = vec![0u16; w * h * 3];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let is_sky = fgnd.is_none_or(|f| f.mask[idx] == SKY);
            let base_e = match (is_sky, fgnd, &p.foreground) {
                (true, _, _) => background_at(p, x as f64, y as f64),
                (false, Some(f), Some(fp)) => {
                    // Near-zero luminance, but textured: this is what a naive
                    // detector trips over.
                    let t = f.texture.sample(x as f64, y as f64);
                    fp.level_e * (1.0 + fp.texture_amplitude * (2.0 * t - 1.0))
                }
                (false, _, _) => 0.0,
            };

            for c in 0..3 {
                let signal_e = plane[idx * 3 + c] as f64 + base_e;
                // Poisson is undefined at λ ≤ 0; below one electron the shot
                // term is meaningless anyway and read noise dominates.
                let shot = if signal_e > 1e-9 {
                    Poisson::new(signal_e)
                        .expect("lambda is finite and > 0 in this branch")
                        .sample(rng)
                } else {
                    0.0
                };
                let e = shot + read.sample(rng);
                let adu = e * p.noise.gain_adu_per_e;
                out[idx * 3 + c] = adu.round().clamp(0.0, 65535.0) as u16;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite;

    /// Tolerance: the stamp is normalised by construction, so the only error is
    /// f64 summation round-off over a few thousand pixels.
    const FLUX_EPS: f64 = 1e-9;

    fn spec_at(x: f64, y: f64, flux_e: f64, fwhm: f64) -> StarSpec {
        StarSpec {
            x,
            y,
            flux_e,
            psf: Psf {
                kind: PsfKind::Gaussian,
                fwhm,
                ellipticity: 0.0,
                theta: 0.0,
            },
            tint: [1.0; 3],
        }
    }

    #[test]
    fn stamp_integrates_to_exactly_the_catalog_flux() {
        let p = suite::test_params("stamp", 1, 64, 64, 1);
        let spec = spec_at(31.5, 30.25, 12_345.0, 3.2);
        let stamp = render_stamp(&spec, &spec.psf, spec.flux_e, &p);
        let total: f64 = stamp.px.iter().sum();
        assert!(
            (total - 12_345.0).abs() < FLUX_EPS * 12_345.0,
            "stamp sums to {total}, expected 12345"
        );
    }

    #[test]
    fn stamp_peak_sits_at_the_true_subpixel_centre() {
        let p = suite::test_params("stamp", 1, 64, 64, 1);
        // Centre exactly on a pixel: the peak pixel must be that pixel.
        let spec = spec_at(32.0, 20.0, 1000.0, 3.0);
        let stamp = render_stamp(&spec, &spec.psf, spec.flux_e, &p);
        let (best, _) = stamp
            .px
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("non-empty stamp");
        let bx = stamp.x0 + (best % stamp.w) as i64;
        let by = stamp.y0 + (best / stamp.w) as i64;
        assert_eq!((bx, by), (32, 20));
    }

    #[test]
    fn tiers_split_the_population_into_thirds() {
        let fluxes: Vec<f64> = (1..=300).map(|i| i as f64).collect();
        let tiers = assign_tiers(&fluxes);
        let count = |t: Tier| tiers.iter().filter(|&&x| x == t).count();
        assert_eq!(count(Tier::Small), 100);
        assert_eq!(count(Tier::Medium), 100);
        assert_eq!(count(Tier::Large), 100);
    }

    #[test]
    fn background_gradient_is_centred_on_the_mean_level() {
        let mut p = suite::test_params("bkg", 1, 101, 101, 1);
        p.background.level_e = 300.0;
        p.background.gradient_x_e = 100.0;
        p.background.gradient_y_e = 0.0;
        assert!((background_at(&p, 50.0, 50.0) - 300.0).abs() < 1e-9);
        assert!((background_at(&p, 0.0, 50.0) - 250.0).abs() < 1e-9);
        assert!((background_at(&p, 100.0, 50.0) - 350.0).abs() < 1e-9);
    }

    #[test]
    fn snr_matches_the_documented_formula() {
        // peak 100 e, background 300 e, read 8 e ⇒ 100 / √(100+300+64).
        let expected = 100.0 / (464.0f64).sqrt();
        assert!((peak_snr(100.0, 300.0, 8.0) - expected).abs() < 1e-12);
    }

    #[test]
    fn saturated_stars_record_the_true_peak_while_the_image_clips() {
        let mut p = suite::test_params("sat", 11, 96, 96, 1);
        p.stars.flux_min_e = 3_000_000.0;
        p.stars.flux_max_e = 3_000_001.0;
        p.noise.read_sigma_e = 0.0;
        let r = render(&p).expect("render");
        let star = &r.catalog.stars[0];
        assert!(star.saturated, "a 3 Me star must be flagged saturated");
        assert!(
            star.peak > 65535.0,
            "truth peak {} must be the pre-clip value",
            star.peak
        );
        assert_eq!(
            *r.image.iter().max().expect("non-empty image"),
            65535,
            "the image itself must clip at 65535"
        );
    }

    /// The `saturated` suite advertises `bleed: true` in its committed params, so
    /// the bleed has to be *measurable* — an earlier length model produced a
    /// column shorter than the saturated core, which meant `bleed: true` rendered
    /// identically to `bleed: false`.
    #[test]
    fn bleed_elongates_the_saturated_core_vertically() {
        let mut p = suite::test_params("bleed", 13, 128, 128, 1);
        p.stars.flux_min_e = 3_000_000.0;
        p.stars.flux_max_e = 3_000_001.0;
        p.stars.fwhm_mean_px = 3.2;
        p.stars.fwhm_sigma_px = 0.0;
        p.stars.tint_strength = 0.0;
        p.noise.read_sigma_e = 0.0;
        p.halo = Some(crate::params::HaloParams {
            flux_threshold_e: 200_000.0,
            flux_frac: 0.12,
            fwhm_factor: 6.0,
            bleed: true,
            bleed_width_px: 2.0,
            bleed_length_gain: 6.0,
        });

        let measure = |p: &Params| {
            let r = render(p).expect("render");
            let s = &r.catalog.stars[0];
            let (cx, cy) = (s.x.round() as usize, s.y.round() as usize);
            let at = |x: usize, y: usize| r.image[(y * 128 + x) * 3] as u32;
            let v = (0..128).filter(|&y| at(cx, y) >= 65530).count();
            let hcount = (0..128).filter(|&x| at(x, cy) >= 65530).count();
            (v, hcount)
        };

        let (v_on, h_on) = measure(&p);
        let mut off = p.clone();
        off.halo.as_mut().expect("halo params").bleed = false;
        let (v_off, h_off) = measure(&off);

        assert_eq!(
            h_on, h_off,
            "bleed must not widen the core horizontally ({h_on} vs {h_off})"
        );
        assert!(
            v_on > v_off,
            "bleed:true produced no vertical elongation ({v_on} px) over bleed:false ({v_off} px)"
        );
        assert!(
            v_on >= 2 * h_on,
            "bleed column ({v_on} px) should clearly exceed the core width ({h_on} px)"
        );
    }

    #[test]
    fn no_star_signal_lands_outside_the_sky_mask_before_noise() {
        let p = suite::nightscape_test_params(23);
        let mut rng = <ChaCha20Rng as rand::SeedableRng>::seed_from_u64(p.seed);
        let fgnd = foreground::build(&p, &mut rng).expect("foreground");
        let specs = field::sample_stars(&p, Some(&fgnd.mask), &mut rng).expect("stars");
        for s in &specs {
            let idx = (s.y.round() as usize) * p.image.width as usize + s.x.round() as usize;
            assert_eq!(fgnd.mask[idx], SKY, "star placed on the foreground");
        }
    }
}
