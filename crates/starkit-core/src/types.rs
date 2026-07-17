//! Catalog schema v1 — **FROZEN** at task T0-3 (2026-07-16).
//!
//! Canonical definition: `docs/FIXTURES.md`, section "Catalog schema v1".
//! Shared contract between `starkit-fixtures` (truth), `oracle/` (measured),
//! and `starkit-core` (detected). Keep this file and that section in lockstep;
//! divergence is a bug.
//!
//! `starkit-fixtures` carries its own mirror of these types rather than
//! depending on this crate (D-006) — truth must not be produced by the code
//! under test. The two definitions are held together by
//! `tests/schema_v1.rs::committed_truth_catalogs_round_trip_byte_identically`,
//! which re-serializes the committed fixtures through *these* types and
//! demands the bytes back unchanged.
//!
//! Round-tripping bytes exactly requires two `serde_json` features, both
//! enabled in the workspace manifest (D-022): `float_roundtrip` (its default
//! float parser is off by 1 ULP on ~5.7 % of the committed values) and
//! `preserve_order` (a `Value` map would otherwise re-emit `generator` keys in
//! alphabetical order).

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Star {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    /// Total flux, linear units.
    pub flux: f64,
    /// Peak pixel value, linear units.
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
    /// `starkit-fixtures`. Opaque here: this crate must not grow an opinion
    /// about how fixtures are made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<serde_json::Value>,
    /// Measurement provenance — method, thresholds and exact library versions.
    /// Present only in *measured* catalogs (the oracle today, `detect` in Phase
    /// 1); the mirror image of [`Catalog::generator`], and equally opaque. A
    /// catalog that outlives its report must still say how it was produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_json_round_trip() {
        let cat = Catalog {
            schema: SCHEMA_V1.to_string(),
            image: ImageMeta {
                width: 4096,
                height: 4096,
                bit_depth: 16,
                color_space: "linear-rgb".into(),
            },
            stars: vec![Star {
                id: 1,
                x: 123.45,
                y: 67.89,
                flux: 15000.0,
                peak: 3200.0,
                fwhm: 3.1,
                ellipticity: 0.04,
                theta: 0.6,
                saturated: false,
                tier: Tier::Small,
                snr: Some(41.2),
            }],
            generator: None,
            measurement: None,
        };

        let json = serde_json::to_string_pretty(&cat).expect("serialize");
        let back: Catalog = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(cat, back);
        assert!(
            json.contains("\"tier\": \"small\""),
            "tier must serialize lowercase"
        );
        assert!(
            !json.contains("generator"),
            "absent generator block must be omitted"
        );
    }
}
