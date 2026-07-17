//! T1-6 acceptance = FR-5 method B: outside-mask bit-identity, determinism, and
//! a golden visual fixture — on the real `basic-5k` and `nightscape-fg` frames,
//! through the full detect → mask → gate → reduce pipeline.
//!
//! This is the first *star operation*, so it is the first real exercise of the
//! rule the rest of Phase 1 depends on: an operation returns a modified plane and
//! only `gate::composite` writes it into an image (starkit-core/CLAUDE.md).
//! Reading `fixtures/` from an integration test is allowed by INV-7.

use starkit_core::detect::{detect, filter_reach_px, DetectParams};
use starkit_core::gate;
use starkit_core::mask::{self, MaskParams, MaskStar};
use starkit_core::reduce::{morphological, MorphParams};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

struct Frame {
    plane: Vec<f32>,
    sky: Option<Vec<f32>>,
    w: usize,
    h: usize,
}

fn load(suite: &str) -> Option<Frame> {
    let root = repo_root().join("fixtures/generated").join(suite);
    let img = root.join("image.tiff");
    if !img.exists() {
        return None;
    }
    let d = starkit_io::decode(&img).expect("decode");
    let plane: Vec<f32> = d
        .pixels
        .chunks_exact(3)
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();
    let sky = {
        let m = root.join("sky_mask.png");
        m.exists()
            .then(|| starkit_io::decode_mask(&m).expect("mask").0)
    };
    Some(Frame {
        plane,
        sky,
        w: d.width as usize,
        h: d.height as usize,
    })
}

fn skip(suite: &str) {
    eprintln!("skipping {suite}: fixture absent — run `cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated`");
}

/// Run the whole pipeline: detect → star mask → gate → erode → composite.
fn reduce_pipeline(f: &Frame) -> (Vec<f32>, gate::Gate) {
    let params = DetectParams {
        fwhm_guess: 3.2,
        saturation: 1.0,
        ..Default::default()
    };
    let mut det = detect(&f.plane, f.w, f.h, &params).expect("detect");
    if let Some(sky) = &f.sky {
        // On a nightscape, drop the silhouette-edge phantoms before masking, or
        // the gate would open on them (D-034).
        det = gate::restrict_to_sky(&det, sky, f.w, f.h, filter_reach_px(3.2) + 3.0)
            .expect("restrict");
    }
    let stars: Vec<MaskStar> = det.iter().map(MaskStar::from).collect();
    let sm = mask::build(&stars, f.w, f.h, &MaskParams::default()).expect("mask");
    let g = gate::build(&sm, f.sky.as_deref(), 2.0).expect("gate");

    let eroded = morphological(&f.plane, f.w, f.h, &MorphParams::default()).expect("erode");
    let out = g.composite(&f.plane, &eroded).expect("composite");
    (out, g)
}

/// **AC: outside-mask bit-identity**, on `basic-5k`.
///
/// Every pixel the gate does not open must be bit-for-bit the input. Zero
/// tolerance — the operation eroded the *whole* plane, so this proves the gate,
/// not the operation, is what protects the rest of the image.
#[test]
fn outside_the_gate_is_bit_identical_on_basic_5k() {
    let Some(f) = load("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let (out, g) = reduce_pipeline(&f);
    let mut touched = 0usize;
    for y in 0..f.h {
        for x in 0..f.w {
            let i = y * f.w + x;
            if g.allowed_at(x, y) == 0.0 && out[i].to_bits() != f.plane[i].to_bits() {
                touched += 1;
            }
        }
    }
    assert_eq!(
        touched, 0,
        "{touched} pixels outside the gate were modified"
    );
}

/// **AC: outside-mask bit-identity**, on `nightscape-fg` — the frame that also
/// has a foreground to protect. A reduction must not touch the trees.
#[test]
fn outside_the_gate_is_bit_identical_on_nightscape_fg() {
    let Some(f) = load("nightscape-fg") else {
        skip("nightscape-fg");
        return;
    };
    assert!(f.sky.is_some(), "nightscape-fg must have a sky mask");
    let (out, g) = reduce_pipeline(&f);

    let mut touched = 0usize;
    let mut foreground_touched = 0usize;
    let sky = f.sky.as_ref().expect("sky");
    for y in 0..f.h {
        for x in 0..f.w {
            let i = y * f.w + x;
            if g.allowed_at(x, y) == 0.0 && out[i].to_bits() != f.plane[i].to_bits() {
                touched += 1;
                if sky[i] <= 0.0 {
                    foreground_touched += 1;
                }
            }
        }
    }
    assert_eq!(
        touched, 0,
        "{touched} pixels outside the gate were modified ({foreground_touched} on the foreground)"
    );
}

/// **AC: determinism.** Same frame, same params, bit-identical output (INV-2).
#[test]
fn reduction_is_bit_identical_across_runs() {
    let Some(f) = load("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let (a, _) = reduce_pipeline(&f);
    let (b, _) = reduce_pipeline(&f);
    assert_eq!(a, b, "the reduction is not deterministic");
}

/// **AC: golden visual fixture.** The reduced output's hash is pinned, so any
/// change to detection, masking, the gate or the erosion moves it and a reviewer
/// must look. Hashed over 16-bit codes — the values a written file would hold.
///
/// A tripwire, not a spec: if this fails, look at the reduced image before you
/// re-pin. Verified to have teeth by `the_reduction_actually_shrinks_stars`,
/// which fails if the operation stops doing anything.
#[test]
fn golden_reduced_output_is_stable_on_basic_5k() {
    const GOLDEN: u64 = 0xcac7_5cc3_1fab_162f;
    let Some(f) = load("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let (out, _) = reduce_pipeline(&f);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in &out {
        let q = (*v as f64 * 65535.0).round().clamp(0.0, 65535.0) as u16;
        for b in q.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    assert_eq!(
        h, GOLDEN,
        "golden reduced output moved ({h:#018x} != {GOLDEN:#018x}). The reduction \
         pipeline changed — verify the reduced image is what you meant before re-pinning."
    );
}

/// The reduction must actually reduce: stars get smaller, and total flux drops
/// because erosion only ever removes. If this fails the golden hash above is
/// vacuous.
#[test]
fn the_reduction_actually_shrinks_stars_on_basic_5k() {
    let Some(f) = load("basic-5k") else {
        skip("basic-5k");
        return;
    };
    let (out, g) = reduce_pipeline(&f);

    // Inside the gate, total flux must fall (erosion is a minimum).
    let (mut before, mut after) = (0.0f64, 0.0f64);
    for y in 0..f.h {
        for x in 0..f.w {
            if g.allowed_at(x, y) > 0.0 {
                let i = y * f.w + x;
                before += f.plane[i] as f64;
                after += out[i] as f64;
            }
        }
    }
    assert!(
        after < before * 0.98,
        "flux inside the gate barely changed ({before:.1} -> {after:.1}); the reduction is not reducing"
    );
    // And nothing brightened anywhere.
    for (o, p) in out.iter().zip(&f.plane) {
        assert!(o <= p, "a pixel brightened: {p} -> {o}");
    }
}
