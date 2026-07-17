//! Star population sampling (FIXTURES.md "Star population").
//!
//! Every draw goes through the one seeded `ChaCha20Rng` in a fixed order, so the
//! population is a pure function of the seed. No `HashMap` appears on this path:
//! the min-separation accelerator is a plain `Vec`-indexed grid.

use rand::RngExt;
use rand_chacha::ChaCha20Rng;
use rand_distr::{Distribution, Normal};

use crate::params::Params;
use crate::psf::{Psf, PsfKind};

/// One star as the renderer needs it: geometry, flux, and colour.
#[derive(Debug, Clone, Copy)]
pub struct StarSpec {
    pub x: f64,
    pub y: f64,
    /// Core flux in electrons (mean across channels).
    pub flux_e: f64,
    pub psf: Psf,
    /// Per-channel multipliers, mean exactly 1.0.
    pub tint: [f64; 3],
}

/// Inverse-transform sample from `p(F) ∝ F^-a` on `[lo, hi]`.
fn sample_power_law(rng: &mut ChaCha20Rng, lo: f64, hi: f64, alpha: f64) -> f64 {
    let u: f64 = rng.random();
    if (alpha - 1.0).abs() < 1e-9 {
        // a = 1 degenerates to log-uniform.
        lo * (hi / lo).powf(u)
    } else {
        let k = 1.0 - alpha;
        (lo.powf(k) + u * (hi.powf(k) - lo.powf(k))).powf(1.0 / k)
    }
}

/// A star's colour: a mild warm/cool tilt, normalised to mean 1.0 so the catalog
/// flux describes the mean-of-channels image exactly (see `catalog::Star`).
fn sample_tint(rng: &mut ChaCha20Rng, strength: f64) -> [f64; 3] {
    if strength <= 0.0 {
        return [1.0; 3];
    }
    // One "temperature" knob in [-1, 1]: negative = blue, positive = red.
    let t: f64 = rng.random_range(-1.0..1.0);
    let raw = [1.0 + strength * t, 1.0, 1.0 - strength * t];
    let mean = (raw[0] + raw[1] + raw[2]) / 3.0;
    [raw[0] / mean, raw[1] / mean, raw[2] / mean]
}

fn sample_fwhm(rng: &mut ChaCha20Rng, p: &Params) -> f64 {
    let normal = Normal::new(p.stars.fwhm_mean_px, p.stars.fwhm_sigma_px)
        .expect("fwhm_sigma_px is a non-negative constant in every suite preset");
    normal.sample(rng).max(p.stars.fwhm_floor_px)
}

fn sample_psf(rng: &mut ChaCha20Rng, p: &Params, fwhm: f64) -> Psf {
    let is_moffat: f64 = rng.random();
    if is_moffat < p.psf.moffat_fraction {
        let beta = rng.random_range(p.psf.moffat_beta_min..p.psf.moffat_beta_max);
        Psf {
            kind: PsfKind::Moffat { beta },
            fwhm,
            ellipticity: 0.0,
            theta: 0.0,
        }
    } else {
        let ellipticity = rng.random_range(0.0..p.psf.ellipticity_max);
        let theta = rng.random_range(0.0..std::f64::consts::PI);
        Psf {
            kind: PsfKind::Gaussian,
            fwhm,
            ellipticity,
            theta,
        }
    }
}

/// Uniform-grid accelerator for the minimum-separation constraint.
struct SepGrid {
    cell: f64,
    cols: usize,
    rows: usize,
    /// Cell index → star indices. Vec-indexed: no hash iteration order anywhere.
    cells: Vec<Vec<(f64, f64)>>,
    min_sep: f64,
}

impl SepGrid {
    fn new(width: f64, height: f64, min_sep: f64) -> Self {
        let cell = min_sep.max(1.0);
        let cols = (width / cell).ceil() as usize + 1;
        let rows = (height / cell).ceil() as usize + 1;
        Self {
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
            min_sep,
        }
    }

    fn accepts(&self, x: f64, y: f64) -> bool {
        if self.min_sep <= 0.0 {
            return true;
        }
        let cx = (x / self.cell) as isize;
        let cy = (y / self.cell) as isize;
        let min_sep2 = self.min_sep * self.min_sep;
        for gy in (cy - 1)..=(cy + 1) {
            for gx in (cx - 1)..=(cx + 1) {
                if gx < 0 || gy < 0 || gx as usize >= self.cols || gy as usize >= self.rows {
                    continue;
                }
                for &(ox, oy) in &self.cells[gy as usize * self.cols + gx as usize] {
                    if (x - ox).powi(2) + (y - oy).powi(2) < min_sep2 {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn insert(&mut self, x: f64, y: f64) {
        let cx = ((x / self.cell) as usize).min(self.cols - 1);
        let cy = ((y / self.cell) as usize).min(self.rows - 1);
        self.cells[cy * self.cols + cx].push((x, y));
    }
}

/// Rejection budget per star. Exceeding it means the suite asks for a density the
/// min-separation constraint cannot satisfy — a params bug, surfaced loudly
/// rather than silently returning a short catalog.
const MAX_PLACEMENT_TRIES: u32 = 10_000;

/// Sample a position honouring the margin, the min-separation constraint, and —
/// for `nightscape-fg` — the sky region.
fn sample_position(
    rng: &mut ChaCha20Rng,
    p: &Params,
    grid: &SepGrid,
    sky_mask: Option<&[u8]>,
) -> Option<(f64, f64)> {
    let m = p.image.margin_px;
    let w = p.image.width as f64;
    let h = p.image.height as f64;
    for _ in 0..MAX_PLACEMENT_TRIES {
        let x: f64 = rng.random_range(m..(w - 1.0 - m));
        let y: f64 = rng.random_range(m..(h - 1.0 - m));
        if let Some(mask) = sky_mask {
            let px = x.round() as usize;
            let py = y.round() as usize;
            if mask[py * p.image.width as usize + px] == 0 {
                continue; // foreground: no stars here, by construction.
            }
        }
        if grid.accepts(x, y) {
            return Some((x, y));
        }
    }
    None
}

/// Sample the full star population for a suite.
///
/// `sky_mask` (255 = sky, 0 = foreground) restricts placement; `None` means the
/// whole frame is sky.
pub fn sample_stars(
    p: &Params,
    sky_mask: Option<&[u8]>,
    rng: &mut ChaCha20Rng,
) -> Result<Vec<StarSpec>, String> {
    let mut grid = SepGrid::new(
        p.image.width as f64,
        p.image.height as f64,
        p.stars.min_separation_px,
    );
    let mut out = Vec::with_capacity(p.stars.count as usize);

    match &p.pairs {
        // `pairs` suite: place `count / 2` pair centres, then split each into two
        // components separated by 0.5–2.0 × FWHM.
        Some(pair) => {
            let n_pairs = p.stars.count / 2;
            for _ in 0..n_pairs {
                let (cx, cy) =
                    sample_position(rng, p, &grid, sky_mask).ok_or_else(|| placement_failure(p))?;
                grid.insert(cx, cy);

                let fwhm_a = sample_fwhm(rng, p);
                let sep =
                    rng.random_range(pair.separation_min_fwhm..pair.separation_max_fwhm) * fwhm_a;
                let angle: f64 = rng.random_range(0.0..std::f64::consts::TAU);
                let (dx, dy) = (0.5 * sep * angle.cos(), 0.5 * sep * angle.sin());

                for (sx, sy, fwhm) in [
                    (cx + dx, cy + dy, fwhm_a),
                    (cx - dx, cy - dy, sample_fwhm(rng, p)),
                ] {
                    out.push(StarSpec {
                        x: sx,
                        y: sy,
                        flux_e: sample_power_law(
                            rng,
                            p.stars.flux_min_e,
                            p.stars.flux_max_e,
                            p.stars.flux_powerlaw_alpha,
                        ),
                        psf: sample_psf(rng, p, fwhm),
                        tint: sample_tint(rng, p.stars.tint_strength),
                    });
                }
            }
        }
        None => {
            for _ in 0..p.stars.count {
                let (x, y) =
                    sample_position(rng, p, &grid, sky_mask).ok_or_else(|| placement_failure(p))?;
                grid.insert(x, y);
                let fwhm = sample_fwhm(rng, p);
                out.push(StarSpec {
                    x,
                    y,
                    flux_e: sample_power_law(
                        rng,
                        p.stars.flux_min_e,
                        p.stars.flux_max_e,
                        p.stars.flux_powerlaw_alpha,
                    ),
                    psf: sample_psf(rng, p, fwhm),
                    tint: sample_tint(rng, p.stars.tint_strength),
                });
            }
        }
    }

    Ok(out)
}

fn placement_failure(p: &Params) -> String {
    format!(
        "suite '{}': could not place a star within {} tries — {} stars at \
         min_separation_px {} in {}×{} is over-dense",
        p.suite,
        MAX_PLACEMENT_TRIES,
        p.stars.count,
        p.stars.min_separation_px,
        p.image.width,
        p.image.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(7)
    }

    #[test]
    fn power_law_samples_stay_in_range_and_favour_the_faint_end() {
        let mut r = rng();
        let mut n_faint = 0;
        for _ in 0..2000 {
            let f = sample_power_law(&mut r, 700.0, 500_000.0, 1.5);
            assert!((700.0..=500_000.0).contains(&f), "sample {f} out of range");
            if f < 7_000.0 {
                n_faint += 1;
            }
        }
        // A bright-tail power law must put most stars near the faint end;
        // the exact fraction is not the point, the ordering is.
        assert!(
            n_faint > 1000,
            "expected a faint-dominated population, got {n_faint}/2000"
        );
    }

    #[test]
    fn tint_has_mean_one_across_channels() {
        let mut r = rng();
        for _ in 0..200 {
            let t = sample_tint(&mut r, 0.2);
            // Tolerance: three f64 divisions by the same mean; round-off only.
            assert!(((t[0] + t[1] + t[2]) / 3.0 - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn zero_tint_strength_is_exactly_neutral() {
        let mut r = rng();
        assert_eq!(sample_tint(&mut r, 0.0), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn min_separation_is_respected() {
        let p = crate::suite::test_params("sep", 3, 256, 256, 200);
        let mut r = rng();
        let stars = sample_stars(&p, None, &mut r).expect("placement");
        let min_sep = p.stars.min_separation_px;
        for (i, a) in stars.iter().enumerate() {
            for b in &stars[i + 1..] {
                let d2 = (a.x - b.x).powi(2) + (a.y - b.y).powi(2);
                assert!(d2 >= min_sep * min_sep, "stars {i} and next are too close");
            }
        }
    }

    #[test]
    fn over_dense_params_fail_loudly_instead_of_returning_a_short_catalog() {
        let mut p = crate::suite::test_params("dense", 3, 64, 64, 5000);
        p.stars.min_separation_px = 20.0;
        let err = sample_stars(&p, None, &mut rng()).expect_err("must not silently truncate");
        assert!(err.contains("over-dense"), "unhelpful error: {err}");
    }
}
