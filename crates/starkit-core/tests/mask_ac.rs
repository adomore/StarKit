//! T1-4 acceptance = FR-4: golden mask hashes stable; pixel-aligned to source.
//!
//! Built from the fixture's **truth catalog**, not from detections: the mask's
//! own correctness is what is under test here, and feeding it detections would
//! mix in T1-3's error budget. Reading `fixtures/` from an integration test is
//! allowed by INV-7.

use starkit_core::mask::{build, build_tiered, MaskParams, MaskStar};
use starkit_core::types::Catalog;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

fn truth(suite: &str) -> Catalog {
    let p = repo_root()
        .join("fixtures/expected")
        .join(suite)
        .join("truth.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("truth.json")).expect("parse")
}

fn stars_of(cat: &Catalog) -> Vec<MaskStar> {
    cat.stars.iter().map(MaskStar::from).collect()
}

/// FNV-1a over the quantised mask.
///
/// Hashing the 16-bit codes the mask will actually be *exported* as, not the f32
/// bits: the deliverable is the file, and an f32 hash would fail on a
/// last-bit change that no exported pixel could show.
fn golden_hash(data: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in data {
        let q = (*v as f64 * 65535.0).round().clamp(0.0, 65535.0) as u16;
        for b in q.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// **AC: golden mask hashes stable.**
///
/// Pinned from a known-good run and hashed over the **16-bit codes the mask
/// exports as**, not its f32 bits: the deliverable is the file, and an f32 hash
/// would trip on a last-bit change no exported pixel could show.
///
/// This is a tripwire, not a specification. Any change to the mask profile, the
/// feather, the halo rule or the export quantisation moves these numbers, and a
/// reviewer then has to decide whether the change was intended. **If you are here
/// because this failed, look at the mask before you update the constant** —
/// re-pinning a golden hash to match new output is how a golden test quietly
/// stops testing anything.
#[test]
fn golden_mask_hashes_are_stable() {
    const GOLDEN: &[(&str, u64)] = &[
        ("basic-5k", 0x17f4_ba9b_b9bb_0fb6),
        ("saturated", 0xd802_008d_537d_435c),
        ("pairs", 0x6d88_9576_9c83_4ef1),
    ];
    let p = MaskParams::default();
    for (suite, want) in GOLDEN {
        let cat = truth(suite);
        let m = build(
            &stars_of(&cat),
            cat.image.width as usize,
            cat.image.height as usize,
            &p,
        )
        .expect("build");
        let got = golden_hash(&m.data);
        assert_eq!(
            got, *want,
            "{suite}: golden mask hash moved ({got:#018x} != {want:#018x}).              The mask changed — verify the change is what you meant before re-pinning."
        );
    }
}

/// **AC: pixel-alignment test vs source dimensions.**
///
/// A mask that is not exactly the source's size cannot be a Photoshop layer mask
/// at all — it is the one property the whole deliverable rests on.
#[test]
fn masks_are_pixel_aligned_to_their_source() {
    for suite in [
        "basic-5k",
        "dense-core",
        "saturated",
        "pairs",
        "nightscape-fg",
    ] {
        let cat = truth(suite);
        let (w, h) = (cat.image.width as usize, cat.image.height as usize);
        let t = build_tiered(&stars_of(&cat), w, h, &MaskParams::default()).expect("build");
        for (name, m) in [
            ("large", &t.large),
            ("medium", &t.medium),
            ("small", &t.small),
            ("combined", &t.combined),
        ] {
            assert_eq!(
                (m.width, m.height),
                (w, h),
                "{suite}/{name}: wrong dimensions"
            );
            assert_eq!(m.data.len(), w * h, "{suite}/{name}: wrong buffer length");
        }
    }
}

/// Rebuilding from the same catalog must give bit-identical masks (INV-2).
#[test]
fn mask_building_is_deterministic_on_a_real_catalog() {
    let cat = truth("basic-5k");
    let (w, h) = (cat.image.width as usize, cat.image.height as usize);
    let s = stars_of(&cat);
    let a = build(&s, w, h, &MaskParams::default()).expect("a");
    let b = build(&s, w, h, &MaskParams::default()).expect("b");
    assert_eq!(a.data, b.data, "mask building is not deterministic");
}

/// Every star in the catalog must actually be covered. A mask that quietly
/// missed stars would look fine and be useless.
#[test]
fn every_catalogued_star_is_covered_on_basic_5k() {
    let cat = truth("basic-5k");
    let (w, h) = (cat.image.width as usize, cat.image.height as usize);
    let m = build(&stars_of(&cat), w, h, &MaskParams::default()).expect("build");
    for s in &cat.stars {
        let (x, y) = (s.x.round() as usize, s.y.round() as usize);
        if x >= w || y >= h {
            continue;
        }
        assert!(
            m.at(x, y) > 0.5,
            "star {} at ({:.1}, {:.1}) is only {} covered",
            s.id,
            s.x,
            s.y,
            m.at(x, y)
        );
    }
}

/// The mask must select stars, not the sky. If it covered most of the frame it
/// would be useless as a selection — and this is the cheapest way to notice a
/// radius that has gone wrong by an order of magnitude.
#[test]
fn the_mask_covers_stars_not_the_whole_sky() {
    let cat = truth("basic-5k");
    let (w, h) = (cat.image.width as usize, cat.image.height as usize);
    let m = build(&stars_of(&cat), w, h, &MaskParams::default()).expect("build");
    let covered = m.data.iter().filter(|v| **v > 0.0).count() as f64 / m.data.len() as f64;
    assert!(
        covered < 0.10,
        "the mask covers {:.1} % of the frame — that is a selection of the sky, not of 5000 stars",
        covered * 100.0
    );
    assert!(covered > 0.001, "the mask covers almost nothing: {covered}");
}

/// The exported file is the deliverable, so the export path is part of the AC:
/// 16-bit grayscale, pixel-aligned, and full coverage lands on 65535 rather than
/// on whatever a tone curve would make of it.
#[test]
fn the_exported_mask_is_16_bit_grayscale_and_pixel_aligned() {
    let cat = truth("saturated");
    let (w, h) = (cat.image.width as usize, cat.image.height as usize);
    let m = build(&stars_of(&cat), w, h, &MaskParams::default()).expect("build");

    let bytes = starkit_io::encode_gray16(
        &m.data,
        w as u32,
        h as u32,
        std::path::Path::new("mask.tiff"),
    )
    .expect("encode");

    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes))
        .expect("decode")
        .with_limits(tiff::decoder::Limits::unlimited());
    assert_eq!(d.dimensions().expect("dims"), (w as u32, h as u32));
    assert_eq!(
        d.colortype().expect("colortype"),
        tiff::ColorType::Gray(16),
        "a layer mask must be 16-bit grayscale"
    );
    let samples = match d.read_image().expect("read") {
        tiff::decoder::DecodingResult::U16(v) => v,
        other => panic!("expected u16, got {other:?}"),
    };
    assert_eq!(samples.len(), w * h);
    assert_eq!(
        *samples.iter().max().expect("non-empty"),
        65535,
        "full coverage must export as 65535 — no curve may be applied to a mask"
    );
}
