//! Catalog schema v1 — local mirror (D-006).
//!
//! `docs/FIXTURES.md` names `starkit-core/src/types.rs` the Rust source of truth
//! for this schema, but the non-circularity rule forbids `starkit-fixtures` from
//! depending on `starkit-core`: truth must not be produced by the code under
//! test. So this crate carries its own serde mirror, and task T0-3 adds the
//! cross-check that the two definitions agree.
//!
//! Keep this file, `starkit-core/src/types.rs`, and the schema section of
//! `docs/FIXTURES.md` in lockstep; divergence is a bug.

use serde::{Deserialize, Serialize};

use crate::params::Params;

/// Schema identifier expected in every catalog file.
pub const SCHEMA_V1: &str = "starkit-catalog/1";

/// Brightness tier. Truth catalogs derive tiers from per-suite flux tertiles;
/// detection may use its own thresholds — evaluation matches by position, never
/// by tier equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Large,
    Medium,
    Small,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    /// e.g. "linear-rgb"
    pub color_space: String,
}

/// One star.
///
/// Coordinates (frozen project-wide): `(0.0, 0.0)` is the **center of the
/// top-left pixel**; x → right, y → down.
///
/// Photometric convention for truth catalogs (D-010): `flux` and `peak` describe
/// the **noiseless, mean-of-RGB-channels** star signal in output ADU, excluding
/// background. Per-star colour tint is normalised to mean 1.0 across channels, so
/// a consumer measuring `(R+G+B)/3` sees exactly these numbers. `peak` is the
/// pre-clip value: a saturated star's true peak is recorded here even though the
/// image clips it at 65535. Halo and bleed components (`saturated` suite) are
/// *additional* signal and are not included in `flux`; see the `generator` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Star {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    /// Total star flux, linear units (ADU).
    pub flux: f64,
    /// Peak pixel value, linear units (ADU), before clipping.
    pub peak: f64,
    /// Full width at half maximum in pixels (geometric mean of the two axes).
    pub fwhm: f64,
    /// `1 - b/a`, in `[0, 1)`.
    pub ellipticity: f64,
    /// Major-axis angle in radians, CCW from +x.
    pub theta: f64,
    pub saturated: bool,
    pub tier: Tier,
    /// Peak signal-to-noise ratio. Required in truth catalogs, optional in
    /// measured/detected catalogs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Must equal [`SCHEMA_V1`].
    pub schema: String,
    pub image: ImageMeta,
    pub stars: Vec<Star>,
    /// Generator parameters + seed — present only in truth catalogs written by
    /// `starkit-fixtures`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<Params>,
}
