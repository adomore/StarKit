//! T1-2 acceptance: background accuracy on `basic-5k`, and determinism.
//!
//! Graded against the fixture's **generator params** — the truth is the number
//! that was used to render the frame, not a second estimate of it.
//!
//! Note on units: the fixture is linear by construction (`color_space:
//! linear-rgb`) but carries no ICC profile, so it is read here straight from the
//! TIFF rather than through `starkit-io`, which would assume sRGB and apply a
//! curve this data never had. See the T1-2 completion note in ROADMAP.md.
//! Reading `fixtures/` from an integration test is allowed by INV-7.

use starkit_core::background::{estimate, BackgroundParams};

const SUITE: &str = "basic-5k";

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

/// The mean-of-RGB plane, normalised to [0, 1] exactly as the fixture stores it
/// (D-010: the catalog describes the mean of the three channels).
/// `None` when the gitignored fixture has not been generated.
fn fixture_plane() -> Option<(Vec<f32>, usize, usize)> {
    let path = repo_root()
        .join("fixtures/generated")
        .join(SUITE)
        .join("image.tiff");
    if !path.exists() {
        return None;
    }
    let bytes = std::fs::read(&path).expect("read fixture");
    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes))
        .expect("tiff")
        .with_limits(tiff::decoder::Limits::unlimited());
    let (w, h) = d.dimensions().expect("dims");
    let samples = match d.read_image().expect("read") {
        tiff::decoder::DecodingResult::U16(v) => v,
        other => panic!("expected u16, got {other:?}"),
    };
    let plane: Vec<f32> = samples
        .chunks_exact(3)
        .map(|c| ((c[0] as f64 + c[1] as f64 + c[2] as f64) / 3.0) as f32)
        .collect();
    Some((plane, w as usize, h as usize))
}

/// The generator params that produced the fixture — the truth being graded
/// against.
fn truth_params() -> Option<serde_json::Value> {
    let path = repo_root()
        .join("fixtures/expected")
        .join(SUITE)
        .join("params.json");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| serde_json::from_str(&s).expect("params.json parses"))
}

/// The background the generator actually laid down at (x, y), in ADU.
///
/// Mirrors `starkit-fixtures`' `background_at`: level plus gradients expressed
/// as the total change across the frame, centred so `level_e` is the mean.
fn truth_background(p: &serde_json::Value, x: usize, y: usize, w: usize, h: usize) -> f64 {
    let bg = &p["background"];
    let level = bg["level_e"].as_f64().expect("level_e");
    let gx = bg["gradient_x_e"].as_f64().expect("gradient_x_e");
    let gy = bg["gradient_y_e"].as_f64().expect("gradient_y_e");
    let gain = p["noise"]["gain_adu_per_e"].as_f64().expect("gain");
    let fx = x as f64 / (w - 1) as f64;
    let fy = y as f64 / (h - 1) as f64;
    (level + gx * (fx - 0.5) + gy * (fy - 0.5)) * gain
}

fn skip(what: &str) {
    eprintln!(
        "skipping {what}: fixtures/generated/{SUITE}/image.tiff absent — run \
         `cargo run --release -p starkit-fixtures -- gen --suite {SUITE} --out fixtures/generated`"
    );
}

/// **AC:** background RMS error within the tolerance documented here, on
/// `basic-5k`.
///
/// **Tolerance: 1.0 ADU RMS**, against a background of ~300 ADU — 0.33 %.
///
/// Where that number comes from, rather than being whatever the code happened to
/// score: the mean-of-RGB plane has a per-pixel noise of
/// √((300 + 8²)/3) ≈ 11.0 ADU. A 64×64 tile holds 4096 samples, so its median
/// has a standard error of ≈ 1.253 × 11.0 / √4096 ≈ 0.215 ADU. The 3×3 grid
/// median and the bilinear interpolation both average further, and the 5000
/// stars are clipped away rather than contributing. 1.0 ADU is therefore ~5×
/// the expected statistical floor: comfortably passable if the estimator is
/// sound, and impossible to pass if it is biased — a 1 % systematic error would
/// be 3 ADU and fail outright.
#[test]
fn background_rms_error_on_basic_5k_is_within_tolerance() {
    const TOLERANCE_ADU: f64 = 1.0;

    let Some((plane, w, h)) = fixture_plane() else {
        skip("background accuracy");
        return;
    };
    let params = truth_params().expect("committed params.json");

    let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");

    let mut sq_sum = 0.0f64;
    let mut n = 0u64;
    let mut worst = 0.0f64;
    for y in (0..h).step_by(7) {
        for x in (0..w).step_by(7) {
            let want = truth_background(&params, x, y, w, h);
            let err = bg.level_at(x, y) as f64 - want;
            sq_sum += err * err;
            worst = worst.max(err.abs());
            n += 1;
        }
    }
    let rms = (sq_sum / n as f64).sqrt();
    assert!(
        rms < TOLERANCE_ADU,
        "background RMS error {rms:.3} ADU exceeds {TOLERANCE_ADU} (worst {worst:.3}, over {n} samples)"
    );
}

/// The noise map must recover the fixture's actual noise.
///
/// Truth: the mean-of-RGB plane averages three independent channels, each with
/// variance (background + read²) in electrons, so σ = √((300 + 64)/3) ≈ 11.02
/// ADU at gain 1. **Tolerance: 5 %** — the clip-bias correction is worth ~4 %
/// at 3σ, so a tolerance any looser would not notice if it were removed.
#[test]
fn noise_map_on_basic_5k_matches_the_generator() {
    const TOLERANCE_FRAC: f64 = 0.05;

    let Some((plane, w, h)) = fixture_plane() else {
        skip("noise map");
        return;
    };
    let params = truth_params().expect("committed params.json");
    let level = params["background"]["level_e"].as_f64().expect("level_e");
    let read = params["noise"]["read_sigma_e"]
        .as_f64()
        .expect("read_sigma_e");
    let gain = params["noise"]["gain_adu_per_e"].as_f64().expect("gain");
    let want = ((level + read * read) / 3.0).sqrt() * gain;

    let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");
    let mean_rms = bg.rms.iter().map(|v| *v as f64).sum::<f64>() / bg.rms.len() as f64;

    assert!(
        (mean_rms - want).abs() / want < TOLERANCE_FRAC,
        "noise map reads {mean_rms:.3} ADU, generator implies {want:.3} \
         ({:+.1} %)",
        (mean_rms / want - 1.0) * 100.0
    );
}

/// **AC:** deterministic. Same input, same params, bit-identical maps (INV-2).
#[test]
fn estimation_is_bit_identical_across_runs_on_basic_5k() {
    let Some((plane, w, h)) = fixture_plane() else {
        skip("determinism");
        return;
    };
    let a = estimate(&plane, w, h, &BackgroundParams::default()).expect("a");
    let b = estimate(&plane, w, h, &BackgroundParams::default()).expect("b");
    assert_eq!(a.level, b.level, "level map is not deterministic");
    assert_eq!(a.rms, b.rms, "rms map is not deterministic");
}

/// The 5000 stars must not lift the background: this is the whole reason for
/// clipping. A plain mean would read high by the stars' total flux spread over
/// the frame.
#[test]
fn the_star_field_does_not_bias_the_background_upward() {
    let Some((plane, w, h)) = fixture_plane() else {
        skip("star bias");
        return;
    };
    let params = truth_params().expect("committed params.json");
    let bg = estimate(&plane, w, h, &BackgroundParams::default()).expect("estimate");

    let mean_est = bg.level.iter().map(|v| *v as f64).sum::<f64>() / bg.level.len() as f64;
    let mean_truth = truth_background(&params, w / 2, h / 2, w, h);
    let plain_mean = plane.iter().map(|v| *v as f64).sum::<f64>() / plane.len() as f64;

    // The naive estimator must actually be wrong here, or this proves nothing.
    assert!(
        plain_mean > mean_truth + 1.0,
        "the plain mean ({plain_mean:.2}) is not biased by stars, so this test is vacuous"
    );
    assert!(
        (mean_est - mean_truth).abs() < 1.0,
        "clipped estimate {mean_est:.3} strayed from truth {mean_truth:.3}"
    );
}
