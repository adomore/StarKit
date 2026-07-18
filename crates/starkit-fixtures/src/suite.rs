//! Suite presets and canonical seeds (FIXTURES.md "Suites (v1)").
//!
//! A suite is nothing but a named [`Params`] preset — the generator itself only
//! ever sees `Params`. That keeps the five committed suites and any ad-hoc
//! params (tests, the T1-9 bench variant) on exactly one code path.
//!
//! **Canonical seeds are project constants (D-007).** They are committed in each
//! suite's `params.json` and pinned by `fixtures/expected/MANIFEST.sha256`.
//! Changing one silently invalidates every committed hash and every quality
//! number ever measured against it: it requires a DECISIONS entry.

use crate::params::*;

/// The five v1 suites, in canonical order.
pub const SUITES: &[&str] = &[
    "basic-5k",
    "dense-core",
    "saturated",
    "pairs",
    "nightscape-fg",
];

/// Canonical seed per suite (D-007). Never change without a D-entry.
///
/// `basic-61mp` has a seed too, but it is **not** one of the committed suites: it
/// is the T1-9 performance variant, generated on demand and never hash-pinned
/// (docs/FIXTURES.md). Keeping it here lets `gen --suite basic-61mp` reproduce
/// the exact frame the baseline was measured on.
pub fn canonical_seed(suite: &str) -> Option<u64> {
    Some(match suite {
        "basic-5k" => 1_000_001,
        "dense-core" => 1_000_002,
        "saturated" => 1_000_003,
        "pairs" => 1_000_004,
        "nightscape-fg" => 1_000_005,
        "basic-61mp" => 1_000_061,
        "basic-100mp" => 1_000_100,
        _ => return None,
    })
}

/// Shared photometric baseline.
///
/// `gain = 1.0` makes electrons and ADU numerically equal, so catalog fluxes read
/// directly against image values with no mental conversion. Background 300 e with
/// read noise 8 e puts the noise floor at √(300 + 64) ≈ 19 e, which sets the flux
/// range each suite needs for its target SNR span.
fn base(suite: &str, seed: u64, width: u32, height: u32, count: u32) -> Params {
    Params {
        schema: PARAMS_SCHEMA_V1.to_string(),
        suite: suite.to_string(),
        seed,
        image: ImageParams {
            width,
            height,
            margin_px: 8.0,
        },
        background: BackgroundParams {
            level_e: 300.0,
            gradient_x_e: 24.0,
            gradient_y_e: -16.0,
        },
        noise: NoiseParams {
            read_sigma_e: 8.0,
            gain_adu_per_e: 1.0,
            saturation_adu: 65535.0,
        },
        psf: PsfParams {
            moffat_fraction: 0.6,
            moffat_beta_min: 2.5,
            moffat_beta_max: 4.5,
            ellipticity_max: 0.15,
            supersample: 8,
            stamp_radius_fwhm: 8.0,
        },
        stars: StarParams {
            count,
            // Peak SNR ≈ 3 at the faint end, ≈ 200 at the bright end for a
            // 3.2 px FWHM Gaussian against the baseline above.
            flux_min_e: 700.0,
            flux_max_e: 500_000.0,
            flux_powerlaw_alpha: 1.5,
            fwhm_mean_px: 3.2,
            fwhm_sigma_px: 0.5,
            fwhm_floor_px: 1.6,
            min_separation_px: 8.0,
            tint_strength: 0.18,
        },
        pairs: None,
        halo: None,
        foreground: None,
    }
}

/// Build the params for a named suite. `None` for an unknown name.
pub fn params_for_suite(suite: &str, seed: u64) -> Option<Params> {
    Some(match suite {
        "basic-5k" => base(suite, seed, 4096, 4096, 5_000),

        // The T1-9 performance variant: `basic-5k` scaled to 61 MP. 9568×6376 is
        // ~3.64× the area of 4096², so the star count is scaled to match the same
        // density (5000 × 3.64 ≈ 18 200). Never committed or hash-pinned — it is
        // generated on demand for the bench only (docs/FIXTURES.md).
        "basic-61mp" => base(suite, seed, 9568, 6376, 18_200),

        // The 100 MP ceiling-validation variant (D-042): Hasselblad X2D 100C
        // dimensions, 11656×8742 ≈ 101.9 MP, density-matched to ~30 000 stars.
        // Same status as basic-61mp — on demand, never committed or hash-pinned.
        "basic-100mp" => base(suite, seed, 11656, 8742, 30_000),

        "dense-core" => {
            let mut p = base(suite, seed, 4096, 4096, 25_000);
            // Milky-Way-core crowding: the deblending stress test.
            p.stars.min_separation_px = 2.0;
            p
        }

        "saturated" => {
            let mut p = base(suite, seed, 2048, 2048, 500);
            // Flux floor raised so ~10 % of the population clips: with α = 1.5 on
            // [2e4, 5e6], P(peak + background ≥ 65535) ≈ 0.11.
            p.stars.flux_min_e = 20_000.0;
            p.stars.flux_max_e = 5_000_000.0;
            p.stars.min_separation_px = 24.0;
            p.halo = Some(HaloParams {
                flux_threshold_e: 200_000.0,
                flux_frac: 0.12,
                fwhm_factor: 6.0,
                bleed: true,
                bleed_width_px: 2.0,
                bleed_length_gain: 6.0,
            });
            p
        }

        "pairs" => {
            let mut p = base(suite, seed, 2048, 2048, 2_000);
            // Pair *centres* keep their distance; the components inside a pair are
            // the point of the suite.
            p.stars.min_separation_px = 24.0;
            p.pairs = Some(PairParams {
                separation_min_fwhm: 0.5,
                separation_max_fwhm: 2.0,
            });
            p
        }

        "nightscape-fg" => {
            let mut p = base(suite, seed, 6144, 4096, 8_000);
            p.foreground = Some(ForegroundParams {
                ridge_base_frac: 0.72,
                ridge_amplitude_frac: 0.06,
                ridge_octaves: 4,
                tree_count: 14,
                tree_depth: 5,
                // ~4 % of sky background: dark, but never zero.
                level_e: 12.0,
                texture_amplitude: 0.6,
                texture_cell_px: 9.0,
            });
            p
        }

        _ => return None,
    })
}

/// Small ad-hoc params for unit tests — kept here so tests and suites share the
/// one preset builder.
#[cfg(test)]
pub fn test_params(label: &str, seed: u64, width: u32, height: u32, count: u32) -> Params {
    let mut p = base(label, seed, width, height, count);
    p.stars.min_separation_px = 4.0;
    p
}

/// Nightscape-shaped params at test scale.
#[cfg(test)]
pub fn nightscape_test_params(seed: u64) -> Params {
    let mut p = test_params("nightscape-test", seed, 192, 128, 60);
    p.foreground = Some(ForegroundParams {
        ridge_base_frac: 0.7,
        ridge_amplitude_frac: 0.06,
        ridge_octaves: 3,
        tree_count: 3,
        tree_depth: 4,
        level_e: 12.0,
        texture_amplitude: 0.6,
        texture_cell_px: 9.0,
    });
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_suite_has_params_and_a_canonical_seed() {
        for s in SUITES {
            assert!(canonical_seed(s).is_some(), "{s} has no canonical seed");
            let p = params_for_suite(s, canonical_seed(s).expect("checked above"))
                .unwrap_or_else(|| panic!("{s} has no params"));
            assert_eq!(p.suite, *s);
            assert_eq!(p.schema, PARAMS_SCHEMA_V1);
        }
    }

    #[test]
    fn canonical_seeds_are_distinct() {
        let mut seeds: Vec<u64> = SUITES.iter().filter_map(|s| canonical_seed(s)).collect();
        seeds.sort_unstable();
        let before = seeds.len();
        seeds.dedup();
        assert_eq!(before, seeds.len(), "two suites share a seed");
    }

    /// The canonical seeds are a committed contract (D-007): this test exists to
    /// fail loudly if someone edits one without going through a D-entry.
    #[test]
    fn canonical_seeds_are_pinned() {
        assert_eq!(canonical_seed("basic-5k"), Some(1_000_001));
        assert_eq!(canonical_seed("dense-core"), Some(1_000_002));
        assert_eq!(canonical_seed("saturated"), Some(1_000_003));
        assert_eq!(canonical_seed("pairs"), Some(1_000_004));
        assert_eq!(canonical_seed("nightscape-fg"), Some(1_000_005));
    }

    #[test]
    fn suite_geometry_matches_the_fixtures_spec_table() {
        let dims = |s: &str| {
            let p = params_for_suite(s, 1).expect("known suite");
            (p.image.width, p.image.height, p.stars.count)
        };
        assert_eq!(dims("basic-5k"), (4096, 4096, 5_000));
        assert_eq!(dims("dense-core"), (4096, 4096, 25_000));
        assert_eq!(dims("saturated"), (2048, 2048, 500));
        assert_eq!(dims("pairs"), (2048, 2048, 2_000));
        assert_eq!(dims("nightscape-fg"), (6144, 4096, 8_000));
    }

    #[test]
    fn unknown_suite_is_rejected() {
        assert!(params_for_suite("no-such-suite", 1).is_none());
        assert!(canonical_seed("no-such-suite").is_none());
    }
}
