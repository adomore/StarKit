//! Generator parameters.
//!
//! A `Params` value plus nothing else fully determines the output bytes: it
//! carries the seed, so `render(&params)` is a pure function of it. Params are
//! emitted next to every fixture (`params.json`) and embedded in the truth
//! catalog's `generator` block, so a fixture can always be traced back to the
//! exact inputs that produced it.
//!
//! Units: `_e` suffix = electrons (the domain where Poisson noise is defined),
//! `_adu` = output digital numbers, `_px` = pixels. `gain_adu_per_e` converts.

use serde::{Deserialize, Serialize};

/// Schema identifier for `params.json`.
pub const PARAMS_SCHEMA_V1: &str = "starkit-fixtures-params/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// Must equal [`PARAMS_SCHEMA_V1`].
    pub schema: String,
    /// Suite name, or a free-form label for ad-hoc params (e.g. in tests).
    pub suite: String,
    /// The single u64 that seeds every random draw in the run.
    pub seed: u64,
    pub image: ImageParams,
    pub background: BackgroundParams,
    pub noise: NoiseParams,
    pub psf: PsfParams,
    pub stars: StarParams,
    /// Present only for the `pairs` suite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairs: Option<PairParams>,
    /// Present only for the `saturated` suite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halo: Option<HaloParams>,
    /// Present only for the `nightscape-fg` suite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ForegroundParams>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageParams {
    pub width: u32,
    pub height: u32,
    /// Star centres are kept this far from the frame border. Stars straddling the
    /// edge are neither detectable nor fairly measurable, so they would poison
    /// the recall metric rather than test anything.
    pub margin_px: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundParams {
    pub level_e: f64,
    /// Total change in electrons across the full width / height.
    pub gradient_x_e: f64,
    pub gradient_y_e: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseParams {
    pub read_sigma_e: f64,
    pub gain_adu_per_e: f64,
    pub saturation_adu: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PsfParams {
    /// Fraction of stars rendered as (circular) Moffat; the rest are elliptical
    /// Gaussians. Per FIXTURES.md "per-suite mix".
    pub moffat_fraction: f64,
    pub moffat_beta_min: f64,
    pub moffat_beta_max: f64,
    /// Upper bound on `1 - b/a` for the Gaussian population.
    pub ellipticity_max: f64,
    /// Sub-samples per axis (×8 per FIXTURES.md).
    pub supersample: u32,
    /// Stamp half-size in units of FWHM.
    pub stamp_radius_fwhm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarParams {
    pub count: u32,
    pub flux_min_e: f64,
    pub flux_max_e: f64,
    /// Exponent `a` of `p(F) ∝ F^-a` — the bright tail.
    pub flux_powerlaw_alpha: f64,
    pub fwhm_mean_px: f64,
    pub fwhm_sigma_px: f64,
    pub fwhm_floor_px: f64,
    pub min_separation_px: f64,
    /// Per-channel tint spread; 0.0 renders every star neutral.
    pub tint_strength: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairParams {
    pub separation_min_fwhm: f64,
    pub separation_max_fwhm: f64,
}

/// Halo / bleed approximation for bright stars. Deliberately not a physical
/// model — it exists so mask code has to cope with saturated-star structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HaloParams {
    /// Stars at or above this core flux get a halo.
    pub flux_threshold_e: f64,
    /// Halo flux as a fraction of core flux (additional to `flux`).
    pub flux_frac: f64,
    /// Halo FWHM as a multiple of the star's core FWHM.
    pub fwhm_factor: f64,
    /// Vertical bleed column on saturated stars (CCD blooming approximation).
    pub bleed: bool,
    pub bleed_width_px: f64,
    /// Stylization factor on the bleed column length (D-013).
    ///
    /// The column length is derived from the star's overflow charge, but at this
    /// suite's brightnesses (peaks ≈ 4–7 × full well) the *physically* implied
    /// column is shorter than the already-saturated core — i.e. invisible. Real
    /// blooming needs far more overexposure, plus well geometry this generator
    /// does not model. This factor scales the column to something a star-mask has
    /// to actually cope with. Not physical, and deliberately named so.
    pub bleed_length_gain: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForegroundParams {
    /// Ridge baseline as a fraction of image height, measured from the top.
    pub ridge_base_frac: f64,
    /// Ridge relief as a fraction of image height.
    pub ridge_amplitude_frac: f64,
    /// Sinusoid octaves summed into the ridgeline.
    pub ridge_octaves: u32,
    pub tree_count: u32,
    pub tree_depth: u32,
    /// Foreground signal level — "near-zero luminance", not zero.
    pub level_e: f64,
    /// Multiplicative texture spread on the foreground level, in `[0, 1)`.
    pub texture_amplitude: f64,
    /// Value-noise cell size in pixels for that texture.
    pub texture_cell_px: f64,
}
