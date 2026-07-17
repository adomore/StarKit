//! FR-1 acceptance criteria (task T1-1).
//!
//! The north star (starkit-io/CLAUDE.md): decode → encode with every operation
//! disabled must produce a **pixel-identical** TIFF16. Everything else in this
//! crate exists to keep that true.
//!
//! These run against the committed `basic-5k` fixture where possible — a real
//! 4096² 16-bit frame with the full dynamic range, saturated stars and noise —
//! rather than a hand-made gradient that would exercise none of the edge cases.
//! Reading `fixtures/` from an integration test is allowed by INV-7.

use std::path::{Path, PathBuf};

use starkit_io::{decode, decode_bytes, encode_tiff16, write_tiff16, IoError};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-io has a grandparent")
        .to_path_buf()
}

/// The real fixture, when it has been generated. `None` otherwise — the images
/// are gitignored, so a fresh clone has none until `gen` runs.
fn fixture() -> Option<PathBuf> {
    let p = repo_root().join("fixtures/generated/basic-5k/image.tiff");
    p.exists().then_some(p)
}

/// A small synthetic TIFF16 that still spans the interesting values: 0, 1, the
/// mid-tones, 65534 and 65535. Used where the big fixture is overkill or absent.
fn synthetic_tiff16() -> (Vec<u8>, Vec<u16>) {
    let (w, h) = (16u32, 8u32);
    let mut samples = Vec::with_capacity((w * h * 3) as usize);
    for i in 0..(w * h) as usize {
        samples.push((i * 517 % 65536) as u16);
        samples.push(if i == 0 {
            0
        } else {
            65535 - (i * 31 % 65536) as u16
        });
        samples.push(match i % 5 {
            0 => 0,
            1 => 1,
            2 => 32768,
            3 => 65534,
            _ => 65535,
        });
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).expect("encoder");
        enc.write_image::<tiff::encoder::colortype::RGB16>(w, h, &samples)
            .expect("write");
    }
    (buf.into_inner(), samples)
}

fn read_tiff16_samples(bytes: &[u8]) -> (u32, u32, Vec<u16>) {
    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes))
        .expect("decode")
        .with_limits(tiff::decoder::Limits::unlimited());
    let (w, h) = d.dimensions().expect("dims");
    match d.read_image().expect("read") {
        tiff::decoder::DecodingResult::U16(v) => (w, h, v),
        other => panic!("expected u16 samples, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AC 1: no-op run is pixel-identical for TIFF16
// ---------------------------------------------------------------------------

/// **AC = FR-1.** Decode and re-encode a synthetic TIFF16 with no operations in
/// between; every one of the 384 samples must survive unchanged, including the
/// 0 / 1 / 65534 / 65535 boundaries where rounding errors show up first.
#[test]
fn noop_round_trip_is_pixel_identical_synthetic() {
    let (bytes, original) = synthetic_tiff16();
    let img = decode_bytes(&bytes, Path::new("synthetic.tiff")).expect("decode");
    let out = encode_tiff16(&img, Path::new("out.tiff")).expect("encode");
    let (_, _, back) = read_tiff16_samples(&out);
    assert_eq!(back, original, "a no-op round trip altered pixels");
}

/// The same claim, on a real 4096² fixture: 50 million samples covering the full
/// range, real Poisson noise and saturated cores. Skipped (loudly) when the
/// fixture has not been generated.
#[test]
fn noop_round_trip_is_pixel_identical_on_the_basic_5k_fixture() {
    let Some(path) = fixture() else {
        eprintln!("skipping: fixtures/generated/basic-5k/image.tiff absent — run `cargo run --release -p starkit-fixtures -- gen --suite basic-5k --out fixtures/generated`");
        return;
    };
    let original = read_tiff16_samples(&std::fs::read(&path).expect("read")).2;
    let img = decode(&path).expect("decode");
    let out = encode_tiff16(&img, Path::new("out.tiff")).expect("encode");
    let (_, _, back) = read_tiff16_samples(&out);

    assert_eq!(back.len(), original.len(), "sample count changed");
    if back != original {
        let (i, (a, b)) = original
            .iter()
            .zip(&back)
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .expect("vectors differ but no differing element");
        let n = original.iter().zip(&back).filter(|(a, b)| a != b).count();
        panic!(
            "{n} of {} samples changed; first at {i}: {a} -> {b}",
            original.len()
        );
    }
}

/// An 8-bit source widens to 16-bit output (FR-1's stated default) and the
/// original 8-bit codes are still exactly recoverable — widening must not
/// perturb, only rescale.
#[test]
fn an_8_bit_source_widens_to_16_bit_without_drift() {
    let (w, h) = (8u32, 4u32);
    let samples: Vec<u8> = (0..(w * h * 3) as usize)
        .map(|i| (i * 7 % 256) as u8)
        .collect();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).expect("encoder");
        enc.write_image::<tiff::encoder::colortype::RGB8>(w, h, &samples)
            .expect("write");
    }
    let img = decode_bytes(&buf.into_inner(), Path::new("in.tiff")).expect("decode");
    assert_eq!(img.meta.source_bit_depth, 8);

    let out = encode_tiff16(&img, Path::new("out.tiff")).expect("encode");
    let (_, _, back) = read_tiff16_samples(&out);
    for (i, &orig) in samples.iter().enumerate() {
        // The 8-bit code must come back at the same position on the 16-bit
        // scale: v/255 == v16/65535, i.e. v16 == v * 257.
        assert_eq!(back[i], orig as u16 * 257, "sample {i} drifted");
    }
}

// ---------------------------------------------------------------------------
// AC 2: corrupt / truncated input fails cleanly, with no partial output
// ---------------------------------------------------------------------------

#[test]
fn a_truncated_tiff_fails_cleanly() {
    let (bytes, _) = synthetic_tiff16();
    let err = decode_bytes(&bytes[..bytes.len() / 3], Path::new("cut.tiff"))
        .expect_err("a truncated TIFF must not decode");
    assert!(
        matches!(err, IoError::Decode { .. }),
        "wrong variant: {err:?}"
    );
}

#[test]
fn a_file_that_is_not_an_image_fails_as_unsupported_not_as_a_bad_tiff() {
    let err = decode_bytes(b"#!/bin/sh\necho hello\n", Path::new("script.sh"))
        .expect_err("must not decode");
    assert!(
        matches!(err, IoError::UnsupportedLayout { .. }),
        "a shell script is not a broken TIFF; got {err:?}"
    );
}

#[test]
fn an_empty_file_fails_cleanly() {
    assert!(decode_bytes(&[], Path::new("empty.tiff")).is_err());
}

#[test]
fn a_missing_file_reports_io_with_its_path() {
    let err = decode("no/such/file.tiff").expect_err("must fail");
    assert!(matches!(err, IoError::Io { .. }), "wrong variant: {err:?}");
    assert!(
        err.to_string().contains("file.tiff"),
        "error must name the path: {err}"
    );
}

/// **AC = FR-1**: "never a partial output file". A failed decode must not have
/// created the output at all.
#[test]
fn a_failed_decode_creates_no_output_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = dir.path().join("bad.tiff");
    std::fs::write(&bad, b"II\x2a\x00garbage").expect("write");
    let out = dir.path().join("out.tiff");

    let result = decode(&bad).and_then(|img| write_tiff16(&img, &out));
    assert!(
        result.is_err(),
        "corrupt input should not have produced output"
    );
    assert!(!out.exists(), "a failed run left a partial output file");
}

/// INV-3: the input file is never modified. Hashing before and after is the only
/// check that actually proves it.
#[test]
fn decoding_never_modifies_the_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("in.tiff");
    let (bytes, _) = synthetic_tiff16();
    std::fs::write(&src, &bytes).expect("write");

    let before = std::fs::read(&src).expect("read");
    let img = decode(&src).expect("decode");
    write_tiff16(&img, dir.path().join("out.tiff")).expect("write out");
    let after = std::fs::read(&src).expect("read");

    assert_eq!(before, after, "the input file was modified");
}

// ---------------------------------------------------------------------------
// AC 3: atomic semantics
// ---------------------------------------------------------------------------

/// **AC = FR-1**: an interrupted write leaves the target untouched.
///
/// A real kill between temp-write and rename cannot be staged in-process, so
/// this asserts the property that makes such a kill safe: the target is only
/// ever touched by the final rename, so any failure before it leaves the
/// previous file byte-for-byte intact. `atomic.rs` covers the failure paths.
#[test]
fn an_interrupted_write_leaves_the_previous_target_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("out.tiff");

    let (bytes, _) = synthetic_tiff16();
    let img = decode_bytes(&bytes, Path::new("in.tiff")).expect("decode");
    write_tiff16(&img, &target).expect("first write");
    let first = std::fs::read(&target).expect("read");

    // Fail after the target exists: the parent of this path is a regular file.
    let doomed = target.join("nested");
    assert!(
        write_tiff16(&img, &doomed).is_err(),
        "write should have failed"
    );

    assert_eq!(
        std::fs::read(&target).expect("read"),
        first,
        "an interrupted write damaged the existing target"
    );
}

#[test]
fn writing_twice_produces_byte_identical_files() {
    // INV-2 at the I/O boundary: same input, same params, same bytes. FR-8's
    // "re-run produces hash-identical outputs" depends on this holding here.
    let dir = tempfile::tempdir().expect("tempdir");
    let (bytes, _) = synthetic_tiff16();
    let img = decode_bytes(&bytes, Path::new("in.tiff")).expect("decode");

    let a = dir.path().join("a.tiff");
    let b = dir.path().join("b.tiff");
    write_tiff16(&img, &a).expect("write a");
    write_tiff16(&img, &b).expect("write b");
    assert_eq!(std::fs::read(&a).expect("a"), std::fs::read(&b).expect("b"));
}

// ---------------------------------------------------------------------------
// Metadata preservation (FR-1 prose)
// ---------------------------------------------------------------------------

#[test]
fn an_untagged_file_is_assumed_srgb_and_says_so() {
    let (bytes, _) = synthetic_tiff16();
    let img = decode_bytes(&bytes, Path::new("in.tiff")).expect("decode");
    assert!(img.meta.icc.is_none());
    assert!(
        img.meta.warnings.iter().any(|w| w.contains("sRGB")),
        "an assumption was made silently: {:?}",
        img.meta.warnings
    );
}

#[test]
fn an_icc_profile_survives_the_round_trip_byte_for_byte() {
    // A minimal gamma-2.2 profile, structured well enough to parse.
    let icc = {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"curv");
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&563u16.to_be_bytes()); // 2.19921875
        let table_at = 128 + 4;
        let data_at = table_at + 12 * 3;
        let mut v = vec![0u8; data_at];
        v.extend_from_slice(&payload);
        let total = v.len() as u32;
        v[0..4].copy_from_slice(&total.to_be_bytes());
        v[128..132].copy_from_slice(&3u32.to_be_bytes());
        for (i, sig) in [b"rTRC", b"gTRC", b"bTRC"].iter().enumerate() {
            let at = table_at + i * 12;
            v[at..at + 4].copy_from_slice(*sig);
            v[at + 4..at + 8].copy_from_slice(&(data_at as u32).to_be_bytes());
            v[at + 8..at + 12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        }
        v
    };

    let (w, h) = (4u32, 2u32);
    let samples: Vec<u16> = (0..(w * h * 3) as usize)
        .map(|i| (i * 4681) as u16)
        .collect();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).expect("encoder");
        let mut im = enc
            .new_image::<tiff::encoder::colortype::RGB16>(w, h)
            .expect("image");
        im.encoder()
            .write_tag(tiff::tags::Tag::Unknown(34675), icc.as_slice())
            .expect("icc tag");
        im.write_data(&samples).expect("data");
    }
    let src = buf.into_inner();

    let img = decode_bytes(&src, Path::new("tagged.tiff")).expect("decode");
    assert_eq!(
        img.meta.icc.as_deref(),
        Some(icc.as_slice()),
        "ICC changed at decode"
    );

    let out = encode_tiff16(&img, Path::new("out.tiff")).expect("encode");
    let img2 = decode_bytes(&out, Path::new("out.tiff")).expect("re-decode");
    assert_eq!(
        img2.meta.icc.as_deref(),
        Some(icc.as_slice()),
        "ICC was not restored"
    );

    // And the pixels still round-trip through a non-sRGB curve.
    let (_, _, back) = read_tiff16_samples(&out);
    assert_eq!(
        back, samples,
        "round trip through a gamma-2.2 profile drifted"
    );
}

// ---------------------------------------------------------------------------
// D-031/D-032: the fixtures now state their own colour space
// ---------------------------------------------------------------------------

/// The integration D-031 was about: a fixture must decode as **linear**.
///
/// Before the fixtures embedded a profile, `starkit-io` followed its documented
/// untagged⇒sRGB rule and applied a tone curve the data never had, so decoded
/// values were not the ADU the truth catalog is written in. Neither component
/// was wrong — the file simply did not say what it was. This asserts the fix
/// end-to-end: decoded == raw/65535, exactly, with no curve in between.
#[test]
fn a_fixture_decodes_as_linear_not_as_srgb() {
    let Some(path) = fixture() else {
        eprintln!("skipping: fixtures/generated/basic-5k/image.tiff absent");
        return;
    };
    let raw = read_tiff16_samples(&std::fs::read(&path).expect("read")).2;
    let img = decode(&path).expect("decode");

    assert!(
        img.meta.icc.is_some(),
        "the fixture carries no ICC profile — regenerate it (D-032)"
    );
    assert!(
        img.meta.warnings.is_empty(),
        "decoding a tagged fixture should need no assumptions, got: {:?}",
        img.meta.warnings
    );

    // Exactness matters: an sRGB curve would put mid-grey at 0.214 instead of
    // 0.5, so a loose tolerance would hide the very bug this guards.
    for (i, (&r, &d)) in raw.iter().zip(&img.pixels).enumerate().step_by(9973) {
        let want = r as f32 / 65535.0;
        assert!(
            (d - want).abs() < 1e-6,
            "sample {i}: decoded {d} but raw/65535 = {want} — a curve was applied"
        );
    }
}
