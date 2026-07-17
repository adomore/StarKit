//! T1-3 acceptance = FR-2 (synthetic), plus the oracle cross-check.
//!
//! Graded against the fixture's **truth catalog** — the positions and widths that
//! were used to render the frame — using the matching rule frozen in
//! docs/FIXTURES.md. The same rule the oracle uses, so the two are comparable.
//!
//! The image is read through `starkit-io`, the real decoder, now that the
//! fixtures state their own colour space (D-032). That is deliberate: grading
//! detection on raw samples would leave the FR-1/FR-2 seam untested exactly where
//! it matters. Reading `fixtures/` from an integration test is allowed by INV-7.

use starkit_core::detect::{detect, DetectParams};

/// docs/FIXTURES.md: recall and precision are conditioned on peak SNR ≥ 5.
///
/// Truth `snr` is **per-channel** (D-017); the mean-of-RGB plane measured here
/// has √3 better SNR, so these stars sit at ≈ 8.7 σ in the data. The bar is
/// honest only when read with that in mind.
const SNR_FLOOR: f64 = 5.0;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

struct Truth {
    x: Vec<f64>,
    y: Vec<f64>,
    fwhm: Vec<f64>,
    flux: Vec<f64>,
    snr: Vec<f64>,
}

fn load(suite: &str) -> Option<(Vec<f32>, usize, usize, Truth)> {
    let root = repo_root();
    let img = root
        .join("fixtures/generated")
        .join(suite)
        .join("image.tiff");
    if !img.exists() {
        return None;
    }

    // Through the real decoder: linear f32, scaled to [0, 1].
    let decoded = starkit_io::decode(&img).expect("decode fixture");
    let plane: Vec<f32> = decoded
        .pixels
        .chunks_exact(3)
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();

    let truth_json = std::fs::read_to_string(
        root.join("fixtures/expected")
            .join(suite)
            .join("truth.json"),
    )
    .expect("truth.json");
    let cat: serde_json::Value = serde_json::from_str(&truth_json).expect("parse truth");
    let stars = cat["stars"].as_array().expect("stars");
    let t = Truth {
        x: stars.iter().map(|s| s["x"].as_f64().expect("x")).collect(),
        y: stars.iter().map(|s| s["y"].as_f64().expect("y")).collect(),
        fwhm: stars
            .iter()
            .map(|s| s["fwhm"].as_f64().expect("fwhm"))
            .collect(),
        flux: stars
            .iter()
            .map(|s| s["flux"].as_f64().expect("flux"))
            .collect(),
        snr: stars
            .iter()
            .map(|s| s["snr"].as_f64().expect("snr"))
            .collect(),
    };
    Some((plane, decoded.width as usize, decoded.height as usize, t))
}

/// Detection params for a fixture: the plane is [0, 1], so saturation is 1.0 and
/// the seeing is the suite's own 3.2 px mean.
fn fixture_params() -> DetectParams {
    DetectParams {
        fwhm_guess: 3.2,
        saturation: 1.0,
        ..Default::default()
    }
}

struct Scored {
    recall: f64,
    precision: f64,
    centroid_median: f64,
    fwhm_median_rel: f64,
    n_detected: usize,
    n_bright: usize,
}

/// The matching rule from docs/FIXTURES.md: a detection matches a truth star if
/// within `max(1.5 px, 0.5 × truth_fwhm)`; greedy in descending truth flux; each
/// truth star matches at most once.
fn score(t: &Truth, det: &[starkit_core::detect::Detection]) -> Scored {
    let n_t = t.x.len();
    let mut matched = vec![usize::MAX; n_t];
    let mut taken = vec![false; det.len()];

    let mut order: Vec<usize> = (0..n_t).collect();
    order.sort_by(|&a, &b| t.flux[b].total_cmp(&t.flux[a]));

    for &i in &order {
        let radius = (0.5 * t.fwhm[i]).max(1.5);
        let r2 = radius * radius;
        let (mut best, mut best_d2) = (usize::MAX, f64::INFINITY);
        for (j, d) in det.iter().enumerate() {
            if taken[j] {
                continue;
            }
            let d2 = (d.x - t.x[i]).powi(2) + (d.y - t.y[i]).powi(2);
            if d2 <= r2 && d2 < best_d2 {
                best = j;
                best_d2 = d2;
            }
        }
        if best != usize::MAX {
            taken[best] = true;
            matched[i] = best;
        }
    }

    let bright: Vec<usize> = (0..n_t).filter(|&i| t.snr[i] >= SNR_FLOOR).collect();
    let found = bright.iter().filter(|&&i| matched[i] != usize::MAX).count();
    let recall = found as f64 / bright.len().max(1) as f64;
    // Precision: a detection landing on ANY real star is a success, not an error
    // — even a star below the floor (D-020, the same definition the oracle uses).
    let precision = taken.iter().filter(|t| **t).count() as f64 / det.len().max(1) as f64;

    let mut cerr: Vec<f64> = Vec::new();
    let mut ferr: Vec<f64> = Vec::new();
    for (i, &j) in matched.iter().enumerate() {
        if j == usize::MAX {
            continue;
        }
        cerr.push(((det[j].x - t.x[i]).powi(2) + (det[j].y - t.y[i]).powi(2)).sqrt());
        if t.fwhm[i] > 0.0 && det[j].fwhm > 0.0 {
            ferr.push((det[j].fwhm - t.fwhm[i]).abs() / t.fwhm[i]);
        }
    }
    let median = |v: &mut Vec<f64>| -> f64 {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };

    Scored {
        recall,
        precision,
        centroid_median: median(&mut cerr),
        fwhm_median_rel: median(&mut ferr),
        n_detected: det.len(),
        n_bright: bright.len(),
    }
}

fn run(suite: &str) -> Option<Scored> {
    let (plane, w, h, t) = load(suite)?;
    let det = detect(&plane, w, h, &fixture_params()).expect("detect");
    Some(score(&t, &det))
}

fn skip(suite: &str) {
    eprintln!(
        "skipping {suite}: fixture absent — run \
         `cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated`"
    );
}

// ---------------------------------------------------------------------------
// AC = FR-2 on basic-5k, the primary metrics suite (docs/FIXTURES.md)
// ---------------------------------------------------------------------------

#[test]
fn fr2_basic_5k_meets_every_bar() {
    let Some(s) = run("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let mut failures = Vec::new();
    if s.recall < 0.98 {
        failures.push(format!("recall {:.4} < 0.98", s.recall));
    }
    if s.precision < 0.99 {
        failures.push(format!("precision {:.4} < 0.99", s.precision));
    }
    if !(s.centroid_median.is_finite() && s.centroid_median <= 0.3) {
        failures.push(format!(
            "median centroid error {:.4} px > 0.3",
            s.centroid_median
        ));
    }
    if !(s.fwhm_median_rel.is_finite() && s.fwhm_median_rel <= 0.10) {
        failures.push(format!("median FWHM error {:.4} > 0.10", s.fwhm_median_rel));
    }
    assert!(
        failures.is_empty(),
        "FR-2 on basic-5k: {}\n  (detected {}, truth at snr>=5: {})",
        failures.join("; "),
        s.n_detected,
        s.n_bright
    );
}

/// **AC:** oracle cross-check — Δrecall ≤ 0.5 pp vs the oracle on `basic-5k`.
///
/// The oracle's number is read from its committed report rather than restated
/// here, so the two can never drift apart silently.
#[test]
fn oracle_cross_check_on_basic_5k() {
    let Some(s) = run("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/expected/reports/basic-5k.json"))
            .expect("committed oracle report"),
    )
    .expect("parse report");
    let oracle_recall = report["metrics"]["recall_at_snr_floor"]
        .as_f64()
        .expect("oracle recall");
    let delta_pp = (s.recall - oracle_recall).abs() * 100.0;

    // The AC's window is 0.5 pp, and it says "deviations beyond that require a
    // DECISIONS entry" — D-033 is that entry. We sit outside the window by
    // *beating* the oracle: 99.94 % against its 99.21 %, Δ = 0.73 pp.
    //
    // Being better than the reference is a good outcome, not a free pass, so the
    // check is split rather than merely widened. Falling below the oracle is still
    // a failure — that is the direction the cross-check exists to catch, since the
    // oracle is the evidence that these stars are findable at all. Drifting far
    // *above* it would mean we are reporting things an independent instrument
    // cannot see, which needs explaining just as much.
    assert!(
        s.recall >= oracle_recall - 0.005,
        "recall {:.4} is more than 0.5 pp BELOW the oracle's {:.4} (Δ = {:.2} pp): \
         the reference finds stars we do not.",
        s.recall,
        oracle_recall,
        delta_pp
    );
    assert!(
        delta_pp <= 2.0,
        "recall {:.4} vs oracle {:.4}: Δ = {:.2} pp. The gap measured at T1-3 was \
         0.73 pp (D-033); a gap this different means one of the two instruments \
         has moved, and which one is the first thing to find out.",
        s.recall,
        oracle_recall,
        delta_pp
    );
}

#[test]
fn detection_is_deterministic_on_basic_5k() {
    let Some((plane, w, h, _)) = load("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let a = detect(&plane, w, h, &fixture_params()).expect("a");
    let b = detect(&plane, w, h, &fixture_params()).expect("b");
    assert_eq!(a, b, "detection is not deterministic");
}

// ---------------------------------------------------------------------------
// The other four suites are stress tests, not bars (D-033).
//
// FR-2's numbers apply to `basic-5k` alone — docs/FIXTURES.md calls it the
// "primary metrics suite", and the other four are labelled deblending-stress,
// deblending-limit, saturation and nightscape. The reading is forced, not
// convenient: the oracle built in T0-2 to calibrate this very bar reaches 98/99
// on none of them either (dense-core 88.1 %, pairs 65.8 %, saturated 96.6 %,
// nightscape 91.6 %), so any other reading makes the AC unachievable by its own
// reference instrument.
//
// They are still pinned here — at what the detector *measures*, with margin —
// so a regression is loud. These are floors, not claims.
// ---------------------------------------------------------------------------

/// Measured 94.19 % recall / 99.51 % precision. Crowding is the point of the
/// suite; the oracle manages 88.10 % recall.
#[test]
fn dense_core_regression_floor() {
    let Some(s) = run("dense-core") else {
        skip("dense-core");
        return;
    };
    assert!(
        s.recall >= 0.90,
        "dense-core recall {:.4} fell below the 0.90 floor",
        s.recall
    );
    assert!(
        s.precision >= 0.97,
        "dense-core precision {:.4} fell below 0.97",
        s.precision
    );
}

/// Measured 99.60 % recall / 95.22 % precision.
///
/// Precision here is what the zero-sum kernel bought: with a plain sum-1
/// Gaussian it was 54.6 %, because shot noise riding on the bright halos cleared
/// a threshold set from the sky. If this floor breaks, that is the first thing to
/// check.
#[test]
fn saturated_regression_floor() {
    let Some(s) = run("saturated") else {
        skip("saturated");
        return;
    };
    assert!(
        s.recall >= 0.97,
        "saturated recall {:.4} fell below 0.97",
        s.recall
    );
    assert!(
        s.precision >= 0.92,
        "saturated precision {:.4} fell below 0.92 — has the detection filter stopped          being zero-sum? Halo shot noise is what this guards.",
        s.precision
    );
}

/// Measured 69.32 % recall.
///
/// Sub-resolution pairs cannot be separated — that is physics, and the point of
/// the suite. The centroid of an unresolved blend sits between its components, so
/// the 0.3 px bar does **not** apply here: at 0.414 px it is measuring the blend
/// honestly. Note the trade — the oracle scores a better centroid (0.086 px) only
/// because it matches fewer, easier stars (65.8 % recall).
#[test]
fn pairs_regression_floor() {
    let Some(s) = run("pairs") else {
        skip("pairs");
        return;
    };
    assert!(
        s.recall >= 0.65,
        "pairs recall {:.4} fell below 0.65",
        s.recall
    );
    assert!(
        s.precision >= 0.94,
        "pairs precision {:.4} fell below 0.94",
        s.precision
    );
}

/// `nightscape-fg` precision is **35.7 %**, and that is expected — D-019/D-034.
///
/// A matched filter measures contrast against the neighbourhood, so ordinary sky
/// beside a black tree silhouette reads as a source. The oracle hit exactly this
/// and went from 59 % to 99.93 % precision only by masking the foreground. Doing
/// that here would put mask gating in two places; INV-1 requires it in exactly
/// one — the `gate` compositor, T1-5.
///
/// So this test pins the *recall* (the sky stars are found) and deliberately does
/// not assert precision. When T1-5 lands it must bring this suite's precision up,
/// and this test should then grow the assertion it is missing.
#[test]
fn nightscape_finds_the_sky_stars_precision_awaits_the_sky_mask_gate() {
    let Some(s) = run("nightscape-fg") else {
        skip("nightscape-fg");
        return;
    };
    assert!(
        s.recall >= 0.94,
        "nightscape-fg recall {:.4} fell below 0.94",
        s.recall
    );
    // Not a bar — a tripwire. If precision ever climbs here without T1-5, the
    // silhouette response has changed and this comment is stale.
    assert!(
        s.precision < 0.90,
        "nightscape-fg precision is now {:.4}: the silhouette false positives are          gone. If T1-5 landed, assert the real bar here and update D-034.",
        s.precision
    );
}

/// The centroid bar holds wherever stars are actually resolved.
#[test]
fn centroid_accuracy_holds_on_the_resolved_suites() {
    // `pairs` is excluded by construction: an unresolved blend's centroid lies
    // between its components, so the number measures the blend, not an error.
    for suite in ["basic-5k", "dense-core", "saturated", "nightscape-fg"] {
        let Some(s) = run(suite) else {
            skip(suite);
            return;
        };
        assert!(
            s.centroid_median <= 0.3,
            "{suite}: median centroid error {:.4} px > 0.3",
            s.centroid_median
        );
    }
}
