//! The performance bench and its regression gate (task T1-9, PRD §6).
//!
//! Times the full compute pipeline on the 61 MP `basic-61mp` variant and checks
//! it against a committed budget. The measured number is **not** hash-pinned —
//! it is a timing, and timings vary by machine and run — so the failing gate is
//! the PRD's machine-independent **hard cap** (20 s), while the reference number
//! is advisory (D-040, and the same cross-machine reasoning as D-012).
//!
//! The pipeline it times is the real one: [`crate::pipeline::process_image`], the
//! exact code `process` runs. Decode and encode are measured but excluded from
//! the gated number — they are disk-bound, and the budget is about the
//! algorithms (PRD §6: "full pipeline (detect + reduce + enhance)").

use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::preset::Preset;

pub const BENCH_SCHEMA_V1: &str = "starkit-bench/1";

/// A committed performance baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub schema: String,
    pub fixture: String,
    pub target_seconds: f64,
    pub hard_cap_seconds: f64,
    /// The last measured reference — informational, updated deliberately with
    /// `--update`, never a pass/fail input by itself.
    pub reference: Reference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub compute_seconds: f64,
    pub megapixels: f64,
    pub stars: usize,
    /// Free-text note on where this was measured — a timing means little without
    /// the machine it was taken on.
    pub measured_on: String,
    pub threads: usize,
}

impl Baseline {
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let b: Baseline = serde_json::from_str(s)?;
        if b.schema != BENCH_SCHEMA_V1 {
            anyhow::bail!("unknown bench baseline schema {:?}", b.schema);
        }
        Ok(b)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("baseline serializes");
        s.push('\n');
        s
    }
}

/// One bench measurement.
pub struct Measurement {
    pub decode_seconds: f64,
    pub compute_seconds: f64,
    pub encode_seconds: f64,
    pub stars: usize,
    pub megapixels: f64,
}

/// Time the pipeline on `input`. `reps` compute runs are taken and the **median**
/// kept, so a single scheduling hiccup does not set the number.
pub fn measure(input: &Path, preset: &Preset, reps: usize) -> anyhow::Result<Measurement> {
    let t = Instant::now();
    let image = starkit_io::decode(input)?;
    let decode_seconds = t.elapsed().as_secs_f64();
    let mp = (image.width as f64 * image.height as f64) / 1e6;

    let reps = reps.max(1);
    let mut times: Vec<f64> = Vec::with_capacity(reps);
    let mut last: Option<(starkit_io::Image, usize)> = None;
    for _ in 0..reps {
        let t = Instant::now();
        let (out, report) = crate::pipeline::process_image(&image, None, preset)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        times.push(t.elapsed().as_secs_f64());
        last = Some((out, report.stars_detected));
    }
    let (out, stars) = last.expect("reps >= 1, so at least one run happened");

    // Encode once — disk cost is reported but not gated (it is I/O-bound).
    let t = Instant::now();
    let _ = starkit_io::encode_tiff16(&out, input)?;
    let encode_seconds = t.elapsed().as_secs_f64();

    Ok(Measurement {
        decode_seconds,
        compute_seconds: median(&mut times),
        encode_seconds,
        stars,
        megapixels: mp,
    })
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Run the bench and apply the gate. Returns the exit code.
pub fn run(input: &Path, baseline_path: &Path, update: bool, reps: usize) -> i32 {
    let preset = Preset::default();

    if !input.exists() {
        eprintln!(
            "starkit bench: {} is absent. Generate it (once, ~1 min):\n  \
             cargo run --release -p starkit-fixtures -- gen --suite basic-61mp --out fixtures/generated",
            input.display()
        );
        return crate::exit::USAGE;
    }

    let m = match measure(input, &preset, reps) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("starkit bench: {e}");
            return crate::exit::PROCESSING;
        }
    };

    println!(
        "bench basic-61mp: {:.1} MP, {} stars\n  \
         decode {:.2}s | compute {:.2}s (median of {}) | encode {:.2}s",
        m.megapixels,
        m.stars,
        m.decode_seconds,
        m.compute_seconds,
        reps.max(1),
        m.encode_seconds
    );

    if update {
        let baseline = Baseline {
            schema: BENCH_SCHEMA_V1.to_string(),
            fixture: "basic-61mp".to_string(),
            target_seconds: 10.0,
            hard_cap_seconds: 20.0,
            reference: Reference {
                compute_seconds: (m.compute_seconds * 100.0).round() / 100.0,
                megapixels: (m.megapixels * 10.0).round() / 10.0,
                stars: m.stars,
                measured_on: "T1-9 reference machine (see D-040)".to_string(),
                threads: 1,
            },
        };
        if let Err(e) = std::fs::write(baseline_path, baseline.to_json()) {
            eprintln!(
                "starkit bench: writing baseline {}: {e}",
                baseline_path.display()
            );
            return crate::exit::IO;
        }
        println!("  baseline updated: {}", baseline_path.display());
        return crate::exit::OK;
    }

    let baseline = match std::fs::read_to_string(baseline_path)
        .map_err(anyhow::Error::from)
        .and_then(|s| Baseline::from_json(&s))
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "starkit bench: reading baseline {}: {e}",
                baseline_path.display()
            );
            return crate::exit::IO;
        }
    };

    // The failing gate is the hard cap — machine-independent (D-040). The target
    // and the reference are reported as context, not enforced.
    let cap = baseline.hard_cap_seconds;
    let ref_s = baseline.reference.compute_seconds;
    if m.compute_seconds <= baseline.target_seconds {
        println!(
            "  within target ({:.0}s). reference was {:.2}s.",
            baseline.target_seconds, ref_s
        );
        crate::exit::OK
    } else if m.compute_seconds <= cap {
        println!(
            "  WARNING over the {:.0}s target but under the {:.0}s cap ({:.2}s). reference {:.2}s.",
            baseline.target_seconds, cap, m.compute_seconds, ref_s
        );
        crate::exit::OK
    } else {
        eprintln!(
            "starkit bench: compute {:.2}s exceeds the {:.0}s hard cap (reference {:.2}s) — a performance regression.",
            m.compute_seconds, cap, ref_s
        );
        crate::exit::BENCH_REGRESSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_round_trips_and_rejects_a_foreign_schema() {
        let b = Baseline {
            schema: BENCH_SCHEMA_V1.to_string(),
            fixture: "basic-61mp".into(),
            target_seconds: 10.0,
            hard_cap_seconds: 20.0,
            reference: Reference {
                compute_seconds: 7.2,
                megapixels: 61.0,
                stars: 18166,
                measured_on: "test".into(),
                threads: 1,
            },
        };
        assert_eq!(Baseline::from_json(&b.to_json()).expect("parse"), b);
        assert!(Baseline::from_json(r#"{"schema":"starkit-bench/2"}"#).is_err());
    }

    #[test]
    fn median_is_the_middle_value() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [5.0]), 5.0);
    }
}
