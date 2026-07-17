//! Versioned processing preset (FR-8).
//!
//! A preset is the *entire* parameter set for one run, in one JSON file, carrying
//! a schema tag so a preset saved today still means the same thing when the tool
//! has moved on. Every field has a default, so a minimal preset — even `{}` — is
//! valid and reproduces the tool's defaults; a caller writes down only what they
//! change.
//!
//! The defaults here are deliberately the same values as the `starkit-core`
//! parameter structs' own `Default` impls. They are restated rather than pulled
//! in so that a preset written to disk is self-contained and explicit — a
//! photographer reading one sees the actual numbers, not a promise that they
//! match some other crate's defaults.

use serde::{Deserialize, Serialize};

use starkit_core::background::BackgroundParams;
use starkit_core::detect::DetectParams;
use starkit_core::mask::MaskParams;
use starkit_core::reduce::{MorphParams, ResynthParams};

pub const PRESET_SCHEMA_V1: &str = "starkit-preset/1";

/// Which reduction method to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Resynthesis — the default (FR-5 method A).
    #[default]
    Resynthesis,
    /// Morphological erosion (FR-5 method B).
    Morphological,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default = "schema_v1")]
    pub schema: String,
    #[serde(default)]
    pub detect: DetectSpec,
    #[serde(default)]
    pub mask: MaskSpec,
    #[serde(default)]
    pub gate: GateSpec,
    #[serde(default)]
    pub reduce: ReduceSpec,
    #[serde(default)]
    pub outputs: OutputSpec,
}

fn schema_v1() -> String {
    PRESET_SCHEMA_V1.to_string()
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            schema: schema_v1(),
            detect: DetectSpec::default(),
            mask: MaskSpec::default(),
            gate: GateSpec::default(),
            reduce: ReduceSpec::default(),
            outputs: OutputSpec::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectSpec {
    pub nsigma: f64,
    pub fwhm_guess: f64,
    pub deblend_contrast: f64,
    /// Saturation level in `[0, 1]` on the linear plane. 1.0 = a fully clipped
    /// 16-bit value.
    pub saturation: f32,
}

impl Default for DetectSpec {
    fn default() -> Self {
        let d = DetectParams::default();
        Self {
            nsigma: d.nsigma,
            fwhm_guess: d.fwhm_guess,
            deblend_contrast: d.deblend_contrast,
            saturation: d.saturation,
        }
    }
}

impl DetectSpec {
    pub fn to_params(&self) -> DetectParams {
        DetectParams {
            nsigma: self.nsigma,
            fwhm_guess: self.fwhm_guess,
            deblend_contrast: self.deblend_contrast,
            saturation: self.saturation,
            background: BackgroundParams::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskSpec {
    pub radius_k: f64,
    pub feather_k: f64,
    pub saturated_halo_k: f64,
}

impl Default for MaskSpec {
    fn default() -> Self {
        let m = MaskParams::default();
        Self {
            radius_k: m.radius_k,
            feather_k: m.feather_k,
            saturated_halo_k: m.saturated_halo_k,
        }
    }
}

impl MaskSpec {
    pub fn to_params(&self) -> MaskParams {
        MaskParams {
            radius_k: self.radius_k,
            feather_k: self.feather_k,
            saturated_halo_k: self.saturated_halo_k,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateSpec {
    pub dilate_px: f64,
}

impl Default for GateSpec {
    fn default() -> Self {
        Self { dilate_px: 2.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReduceSpec {
    pub method: Method,
    /// Target FWHM fraction `r ∈ [0, 1]` for resynthesis.
    pub reduction: f64,
    pub protect_saturated: bool,
    /// Morphological structuring-element radius (used only for that method).
    pub morph_radius: f64,
    pub morph_iterations: usize,
}

impl Default for ReduceSpec {
    fn default() -> Self {
        let r = ResynthParams::default();
        let m = MorphParams::default();
        Self {
            method: Method::default(),
            reduction: r.reduction,
            protect_saturated: r.protect_saturated,
            morph_radius: m.radius,
            morph_iterations: m.iterations,
        }
    }
}

impl ReduceSpec {
    pub fn to_resynth(&self) -> ResynthParams {
        ResynthParams {
            reduction: self.reduction,
            protect_saturated: self.protect_saturated,
            ..ResynthParams::default()
        }
    }

    pub fn to_morph(&self) -> MorphParams {
        MorphParams {
            radius: self.morph_radius,
            iterations: self.morph_iterations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    /// Write the reduced image. On by default — it is the point of a run.
    pub reduced: bool,
    /// Also export the combined star mask as 16-bit grayscale (FR-4).
    pub mask: bool,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            reduced: true,
            mask: false,
        }
    }
}

impl Preset {
    /// Parse a preset from JSON, rejecting an unknown schema tag rather than
    /// silently misinterpreting fields that may have changed meaning.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let p: Preset = serde_json::from_str(s)?;
        if p.schema != PRESET_SCHEMA_V1 {
            anyhow::bail!(
                "unknown preset schema {:?} (this build understands {:?})",
                p.schema,
                PRESET_SCHEMA_V1
            );
        }
        Ok(p)
    }

    pub fn dilate_px(&self) -> f64 {
        self.gate.dilate_px
    }
}

/// The reach a nightscape needs excluded around the foreground, derived from the
/// detection filter so the two cannot drift (D-036).
pub fn sky_margin_px(detect: &DetectSpec) -> f64 {
    starkit_core::detect::filter_reach_px(detect.fwhm_guess) + 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_object_is_a_valid_preset_of_all_defaults() {
        let p = Preset::from_json("{}").expect("empty preset");
        assert_eq!(p, Preset::default());
    }

    #[test]
    fn a_partial_preset_overrides_only_what_it_names() {
        let p = Preset::from_json(r#"{"reduce": {"method": "morphological", "reduction": 0.3, "protect_saturated": false, "morph_radius": 2.0, "morph_iterations": 3}}"#)
            .expect("partial preset");
        assert_eq!(p.reduce.method, Method::Morphological);
        assert_eq!(p.reduce.reduction, 0.3);
        // Untouched sections keep their defaults.
        assert_eq!(p.detect, DetectSpec::default());
        assert_eq!(p.mask, MaskSpec::default());
    }

    #[test]
    fn a_foreign_schema_is_rejected() {
        assert!(Preset::from_json(r#"{"schema": "starkit-preset/2"}"#).is_err());
    }

    #[test]
    fn preset_round_trips_through_json() {
        let p = Preset::default();
        let s = serde_json::to_string_pretty(&p).expect("serialize");
        assert_eq!(Preset::from_json(&s).expect("parse"), p);
    }

    #[test]
    fn preset_defaults_match_the_core_param_defaults() {
        // The whole point of restating the defaults is that they agree; if a core
        // default changes and this is not updated, this test fails.
        let p = Preset::default();
        assert_eq!(p.detect.to_params(), DetectParams::default());
        assert_eq!(p.mask.to_params(), MaskParams::default());
        assert_eq!(p.reduce.to_resynth(), ResynthParams::default());
    }
}
