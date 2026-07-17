//! Per-image processing report (FR-8).
//!
//! One JSON object per processed image: what was done, how many stars, which
//! parameters, and any warnings. Written next to the output so a batch leaves an
//! auditable trail.
//!
//! **Timings are deliberately not in this struct.** FR-8 requires re-runs to
//! produce hash-identical *outputs*, and a report is an output too — a
//! wall-clock timing would make every report differ between runs and defeat a
//! byte-comparison. The batch driver prints elapsed time to the log instead,
//! where it informs without being pinned.

use serde::{Deserialize, Serialize};

use crate::preset::{Method, Preset};

pub const REPORT_SCHEMA_V1: &str = "starkit-report/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_output: Option<String>,
    pub status: Status,
    pub stars_detected: usize,
    /// Fraction of the frame the gate opened — how much of the image the
    /// reduction was allowed to touch.
    pub gate_open_fraction: f64,
    pub reduction: ReductionInfo,
    /// Non-fatal observations from decoding (e.g. an untagged file assumed sRGB).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReductionInfo {
    pub method: Method,
    pub requested_r: f64,
}

impl Report {
    pub fn new(
        stars_detected: usize,
        gate_open_fraction: f64,
        preset: &Preset,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            schema: REPORT_SCHEMA_V1.to_string(),
            input: String::new(),
            output: None,
            mask_output: None,
            status: Status::Ok,
            stars_detected,
            gate_open_fraction,
            reduction: ReductionInfo {
                method: preset.reduce.method,
                requested_r: preset.reduce.reduction,
            },
            warnings,
        }
    }

    /// A report for an image that failed and was skipped (FR-8).
    pub fn skipped(input: &str, reason: &str) -> Self {
        Self {
            schema: REPORT_SCHEMA_V1.to_string(),
            input: input.to_string(),
            output: None,
            mask_output: None,
            status: Status::Skipped,
            stars_detected: 0,
            gate_open_fraction: 0.0,
            reduction: ReductionInfo {
                method: Method::Resynthesis,
                requested_r: 0.0,
            },
            warnings: vec![reason.to_string()],
        }
    }

    /// Serialize with the workspace JSON policy: LF, one trailing newline.
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("a report always serializes");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_round_trips() {
        let r = Report::new(4994, 0.05, &Preset::default(), vec!["no ICC".into()]);
        let json = r.to_json();
        let back: Report = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, r);
    }

    #[test]
    fn a_report_carries_no_timing_so_reruns_match() {
        // The whole struct is a pure function of its inputs — no clock anywhere —
        // so two reports built from the same inputs are byte-identical.
        let a = Report::new(10, 0.1, &Preset::default(), vec![]);
        let b = Report::new(10, 0.1, &Preset::default(), vec![]);
        assert_eq!(a.to_json(), b.to_json());
    }

    #[test]
    fn a_skipped_report_is_marked_skipped_and_names_the_reason() {
        let r = Report::skipped("bad.tif", "truncated file");
        assert_eq!(r.status, Status::Skipped);
        assert!(r.to_json().contains("truncated file"));
        assert!(r.to_json().contains("\"status\": \"skipped\""));
    }

    #[test]
    fn json_uses_lf_and_one_trailing_newline() {
        let s = Report::new(1, 0.0, &Preset::default(), vec![]).to_json();
        assert!(!s.contains('\r'));
        assert!(s.ends_with("}\n") && !s.ends_with("}\n\n"));
    }
}
