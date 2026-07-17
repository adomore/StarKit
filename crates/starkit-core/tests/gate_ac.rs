//! T1-5 acceptance = FR-3's manual path, on the real `nightscape-fg` fixture.
//!
//! Two things are proved here:
//!
//! 1. **INV-1**: pixels outside the sky mask are bit-identical to the input, on a
//!    frame with a real ridgeline and 14 procedural trees — the case a
//!    hand-written unit test cannot invent.
//! 2. **D-034's debt**: `detect` scored 35.7 % precision on this suite because a
//!    matched filter reads sky-beside-silhouette as a source. T1-5 owed this suite
//!    a real precision assertion, and this is it.
//!
//! The sky mask is loaded through `starkit-io::decode_mask` — the same import path
//! a photographer's hand-painted PS mask takes (FR-3's manual path is the
//! *guarantee*, so it is what gets tested).

use starkit_core::detect::{detect, DetectParams};
use starkit_core::gate;
use starkit_core::mask::{self, MaskParams, MaskStar};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

struct Frame {
    plane: Vec<f32>,
    sky: Vec<f32>,
    w: usize,
    h: usize,
}

fn nightscape() -> Option<Frame> {
    let root = repo_root().join("fixtures/generated/nightscape-fg");
    let img = root.join("image.tiff");
    let msk = root.join("sky_mask.png");
    if !img.exists() || !msk.exists() {
        return None;
    }
    let d = starkit_io::decode(&img).expect("decode image");
    let plane: Vec<f32> = d
        .pixels
        .chunks_exact(3)
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();
    let (sky, mw, mh) = starkit_io::decode_mask(&msk).expect("decode sky mask");
    assert_eq!(
        (mw, mh),
        (d.width, d.height),
        "the sky mask must be pixel-aligned to its image"
    );
    Some(Frame {
        plane,
        sky,
        w: d.width as usize,
        h: d.height as usize,
    })
}

fn skip() {
    eprintln!(
        "skipping: fixtures/generated/nightscape-fg absent — run \
         `cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated`"
    );
}

fn detected(f: &Frame) -> Vec<starkit_core::detect::Detection> {
    detect(
        &f.plane,
        f.w,
        f.h,
        &DetectParams {
            fwhm_guess: 3.2,
            saturation: 1.0,
            ..Default::default()
        },
    )
    .expect("detect")
}

/// **AC:** the invariant test — pixels outside the sky mask are bit-identical to
/// input, on `nightscape-fg`.
///
/// The "modified" plane is deliberately absurd (a constant far outside the data's
/// range). If a single foreground pixel is touched, it cannot hide in a rounding
/// difference — it will be blatant. Zero tolerance, as
/// starkit-core/CLAUDE.md requires outside a mask.
#[test]
fn foreground_pixels_are_bit_identical_on_nightscape_fg() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let stars: Vec<MaskStar> = detected(&f).iter().map(MaskStar::from).collect();
    let sm = mask::build(&stars, f.w, f.h, &MaskParams::default()).expect("mask");
    let g = gate::build(&sm, Some(&f.sky), 3.0).expect("gate");

    let modified = vec![0.5f32; f.w * f.h];
    let out = g.composite(&f.plane, &modified).expect("composite");

    let touched = out
        .iter()
        .zip(&f.plane)
        .zip(&f.sky)
        .filter(|((o, p), s)| **s <= 0.0 && o.to_bits() != p.to_bits())
        .count();
    assert_eq!(
        touched, 0,
        "{touched} foreground pixels were modified — INV-1 is broken"
    );
}

/// INV-1 must hold even when the operation returns garbage. An operation that
/// divides by a zero background is a plausible bug; it must not be able to put a
/// NaN on the photographer's trees.
#[test]
fn a_broken_operation_cannot_damage_the_foreground() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let stars: Vec<MaskStar> = detected(&f).iter().map(MaskStar::from).collect();
    let sm = mask::build(&stars, f.w, f.h, &MaskParams::default()).expect("mask");
    let g = gate::build(&sm, Some(&f.sky), 3.0).expect("gate");

    let poison = vec![f32::NAN; f.w * f.h];
    let out = g.composite(&f.plane, &poison).expect("composite");

    for (i, ((o, p), s)) in out.iter().zip(&f.plane).zip(&f.sky).enumerate() {
        if *s <= 0.0 {
            assert!(
                !o.is_nan() && o.to_bits() == p.to_bits(),
                "a NaN reached foreground pixel {i}"
            );
        }
    }
}

/// The gate must actually open somewhere, or the invariant above is vacuous: a
/// gate that is closed everywhere trivially never touches the foreground.
#[test]
fn the_gate_opens_on_sky_stars_so_the_invariant_is_not_vacuous() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let stars: Vec<MaskStar> = detected(&f).iter().map(MaskStar::from).collect();
    let sm = mask::build(&stars, f.w, f.h, &MaskParams::default()).expect("mask");
    let g = gate::build(&sm, Some(&f.sky), 3.0).expect("gate");

    let open = g.open_fraction();
    assert!(
        open > 0.005,
        "the gate opens on only {:.3} % of the frame — the invariant test is vacuous",
        open * 100.0
    );
    // And it must still be a selection of stars, not of the sky.
    assert!(
        open < 0.30,
        "the gate opens on {:.1} % of the frame — that is not a star selection",
        open * 100.0
    );
}

/// **D-034's debt, paid.**
///
/// `detect` alone scores 35.7 % precision here: a matched filter measures contrast
/// against its neighbourhood, and ordinary sky beside a black tree silhouette wins
/// that comparison, so phantom stars appear along every branch. Restricting to the
/// sky with a margin — the same fix the oracle needed (D-019: 59 % → 99.93 %) —
/// removes them.
///
/// The margin covers the detection kernel's reach: the response peaks on *sky*
/// pixels a few px from the branch, so a centre-in-sky test keeps exactly the
/// artifacts this exists to drop.
#[test]
fn sky_restriction_recovers_precision_on_nightscape_fg() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let cat: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/expected/nightscape-fg/truth.json"))
            .expect("truth.json"),
    )
    .expect("parse");
    let stars = cat["stars"].as_array().expect("stars");
    let tx: Vec<f64> = stars.iter().map(|s| s["x"].as_f64().expect("x")).collect();
    let ty: Vec<f64> = stars.iter().map(|s| s["y"].as_f64().expect("y")).collect();
    let tf: Vec<f64> = stars
        .iter()
        .map(|s| s["fwhm"].as_f64().expect("fwhm"))
        .collect();
    let tfl: Vec<f64> = stars
        .iter()
        .map(|s| s["flux"].as_f64().expect("flux"))
        .collect();

    // Margin = the detection filter's own reach, plus 3 px.
    //
    // The reach (5 px at this seeing) is where the kernel footprint can still
    // touch a branch. The extra 3 px is measured, not padding: the surviving
    // false positives sit a median of 3.4 px from foreground with a p99 of 6.0,
    // because a maximum triggered at the reach limit still gets a centroid a
    // little further out. Measured precision by margin: 4 px → 0.73, 6 px → 0.99,
    // 8 px → 0.9991, against the oracle's 0.9993.
    let margin = starkit_core::detect::filter_reach_px(3.2) + 3.0;
    let raw = detected(&f);
    let kept = gate::restrict_to_sky(&raw, &f.sky, f.w, f.h, margin).expect("restrict");

    // Precision by the FIXTURES.md matching rule (D-020's definition).
    let precision = |det: &[starkit_core::detect::Detection]| -> f64 {
        let mut taken = vec![false; det.len()];
        let mut order: Vec<usize> = (0..tx.len()).collect();
        order.sort_by(|&a, &b| tfl[b].total_cmp(&tfl[a]));
        for &i in &order {
            let r = (0.5 * tf[i]).max(1.5);
            let r2 = r * r;
            let (mut best, mut bd) = (usize::MAX, f64::INFINITY);
            for (j, d) in det.iter().enumerate() {
                if taken[j] {
                    continue;
                }
                let d2 = (d.x - tx[i]).powi(2) + (d.y - ty[i]).powi(2);
                if d2 <= r2 && d2 < bd {
                    best = j;
                    bd = d2;
                }
            }
            if best != usize::MAX {
                taken[best] = true;
            }
        }
        taken.iter().filter(|t| **t).count() as f64 / det.len().max(1) as f64
    };

    let before = precision(&raw);
    let after = precision(&kept);

    // The test would be vacuous if detect had quietly stopped producing the
    // artifacts: assert the problem exists before asserting it is fixed.
    assert!(
        before < 0.60,
        "detect's raw precision is {before:.4}, not the ~0.36 D-034 recorded — \
         the silhouette response changed and D-034 needs revisiting"
    );
    assert!(
        after >= 0.95,
        "sky restriction lifted precision only to {after:.4} (from {before:.4}); \
         the oracle reaches 0.9993 with the same fix (D-019)"
    );
}

/// Restricting to the sky must not cost the sky's own stars.
#[test]
fn sky_restriction_keeps_the_stars_that_are_actually_in_the_sky() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let margin = starkit_core::detect::filter_reach_px(3.2) + 3.0;
    let raw = detected(&f);
    let kept = gate::restrict_to_sky(&raw, &f.sky, f.w, f.h, margin).expect("restrict");

    // Every kept detection must sit on sky.
    for d in &kept {
        let i = (d.y.round() as usize).min(f.h - 1) * f.w + (d.x.round() as usize).min(f.w - 1);
        assert!(f.sky[i] > 0.0, "a kept detection is on the foreground");
    }
    // Measured: 6919 survive of 20704 raw, against 8000 truth sky stars —
    // recall 0.8897, where the oracle gets 0.9163 at a 3 px margin.
    //
    // That gap is a trade, not a defect, and it runs the other way on clean
    // fields. Our kernel truncates at 3σ where DAOFIND stops at 1.5σ: the wider
    // kernel is why `basic-5k` recall is 99.94 % against the oracle's 99.21 %,
    // and it is also why the silhouette margin has to be wider, which costs sky
    // stars near the trees. Buying precision with recall here is the right way
    // round — a phantom star gets *operated on*, while a missed faint one is
    // simply left alone.
    assert!(
        kept.len() > 6000,
        "only {} detections survived — the restriction is eating real sky stars",
        kept.len()
    );
}

/// A mask for a different image must be refused, not resampled. Silently
/// stretching it would gate the wrong pixels — the one thing INV-1 exists to
/// prevent.
#[test]
fn a_sky_mask_from_a_different_image_is_refused() {
    let Some(f) = nightscape() else {
        skip();
        return;
    };
    let stars: Vec<MaskStar> = Vec::new();
    let sm = mask::build(&stars, f.w, f.h, &MaskParams::default()).expect("mask");
    let wrong_size = vec![1.0f32; (f.w - 1) * f.h];
    assert!(
        gate::build(&sm, Some(&wrong_size), 2.0).is_err(),
        "a mismatched sky mask was accepted"
    );
}
