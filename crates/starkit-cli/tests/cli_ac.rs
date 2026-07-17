//! T1-8 acceptance = FR-8, driven through the compiled `starkit` binary so the
//! stable exit-code table is exercised, not just the library logic.
//!
//! FR-8's three bars:
//!   1. a 100-image batch runs unattended,
//!   2. a re-run produces hash-identical outputs,
//!   3. one poisoned file is reported and skipped without aborting the batch.
//!
//! Uses tiny synthetic TIFFs: the AC is about the batch *mechanism*, and 100
//! full-size frames would be gigabytes for no extra coverage.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The binary under test — cargo sets this for integration tests.
const BIN: &str = env!("CARGO_BIN_EXE_starkit");

/// Exit codes, mirrored from `src/exit.rs` (kept in sync by hand — that file is
/// the source of truth, this is a test's view of it).
const OK: i32 = 0;
const DECODE: i32 = 4;
const BATCH_PARTIAL: i32 = 7;

fn sha256(path: &Path) -> String {
    // A tiny dependency-free FNV is enough to compare two files for equality.
    let bytes = std::fs::read(path).expect("read output");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Write a minimal RGB16 TIFF with a few bright blobs, via the `tiff` crate.
fn write_tiny_tiff(path: &Path, w: u32, h: u32, seed: u32) {
    let (wu, hu) = (w as usize, h as usize);
    let mut px = vec![600u16; wu * hu * 3]; // a flat background
                                            // A handful of stars at seed-dependent positions.
    for k in 0..4u32 {
        let cx = ((seed.wrapping_mul(2654435761).wrapping_add(k * 40)) % (w - 8) + 4) as usize;
        let cy = ((seed.wrapping_mul(40503).wrapping_add(k * 27)) % (h - 8) + 4) as usize;
        for dy in -2i64..=2 {
            for dx in -2i64..=2 {
                let x = cx as i64 + dx;
                let y = cy as i64 + dy;
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let peak = 40000u16.saturating_sub((dx.abs() + dy.abs()) as u16 * 4000);
                let i = (y as usize * wu + x as usize) * 3;
                for c in 0..3 {
                    px[i + c] = px[i + c].max(peak);
                }
            }
        }
    }
    let f = std::fs::File::create(path).expect("create tiff");
    let mut enc = tiff::encoder::TiffEncoder::new(std::io::BufWriter::new(f)).expect("encoder");
    enc.write_image::<tiff::encoder::colortype::RGB16>(w, h, &px)
        .expect("write image");
}

fn tiffs_in(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(pattern))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

/// **AC: a 100-image batch runs unattended; a poisoned file is reported and
/// skipped; the batch does not abort.**
#[test]
fn a_100_image_batch_skips_one_poisoned_file_and_finishes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let indir = tmp.path().join("in");
    let outdir = tmp.path().join("out");
    std::fs::create_dir_all(&indir).expect("mkdir in");

    for i in 0..100 {
        write_tiny_tiff(&indir.join(format!("img_{i:03}.tiff")), 48, 48, i);
    }
    // One poisoned file: a .tiff that is not a TIFF.
    std::fs::write(indir.join("img_poison.tiff"), b"not a tiff at all").expect("poison");

    let status = Command::new(BIN)
        .args(["batch", "--input-dir"])
        .arg(&indir)
        .arg("--out-dir")
        .arg(&outdir)
        .status()
        .expect("run batch");

    // The batch finished with the partial code, not a crash and not success.
    assert_eq!(
        status.code(),
        Some(BATCH_PARTIAL),
        "batch should exit BATCH_PARTIAL when one file is skipped"
    );

    // All 100 good images produced a reduced output.
    let outputs = tiffs_in(&outdir, "starkit.tiff");
    assert_eq!(
        outputs.len(),
        100,
        "expected 100 reduced outputs, got {}",
        outputs.len()
    );

    // Every input got a report, including the poisoned one.
    let reports = std::fs::read_dir(&outdir)
        .expect("read out")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().to_string_lossy().ends_with("report.json"))
        .count();
    assert_eq!(
        reports, 101,
        "expected 101 reports (100 ok + 1 skipped), got {reports}"
    );

    // The poisoned file's report says skipped and names a reason.
    let poison_report =
        std::fs::read_to_string(outdir.join("img_poison.report.json")).expect("poison report");
    assert!(
        poison_report.contains("\"status\": \"skipped\""),
        "poisoned file not marked skipped: {poison_report}"
    );
}

/// **AC: a re-run produces hash-identical outputs** (INV-2 through the CLI).
#[test]
fn a_rerun_produces_hash_identical_outputs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let indir = tmp.path().join("in");
    std::fs::create_dir_all(&indir).expect("mkdir");
    for i in 0..8 {
        write_tiny_tiff(&indir.join(format!("img_{i}.tiff")), 64, 64, i * 7 + 1);
    }

    let run = |out: &str| -> Vec<String> {
        let outdir = tmp.path().join(out);
        let status = Command::new(BIN)
            .args(["batch", "--input-dir"])
            .arg(&indir)
            .arg("--out-dir")
            .arg(&outdir)
            .status()
            .expect("run");
        assert_eq!(status.code(), Some(OK), "clean batch should exit OK");
        tiffs_in(&outdir, "starkit.tiff")
            .iter()
            .map(|p| sha256(p))
            .collect()
    };

    let first = run("out1");
    let second = run("out2");
    assert_eq!(first.len(), 8);
    assert_eq!(
        first, second,
        "re-run produced different outputs — not deterministic"
    );
}

/// A single clean `process` exits OK and writes a decodable 16-bit TIFF.
#[test]
fn process_writes_a_reduced_image_and_exits_ok() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = tmp.path().join("in.tiff");
    let output = tmp.path().join("out.tiff");
    let report = tmp.path().join("r.json");
    write_tiny_tiff(&input, 96, 96, 3);

    let status = Command::new(BIN)
        .args(["process", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .arg("--report")
        .arg(&report)
        .status()
        .expect("run process");
    assert_eq!(status.code(), Some(OK));
    assert!(output.exists(), "no reduced image written");

    // It must be a real 16-bit RGB TIFF.
    let bytes = std::fs::read(&output).expect("read output");
    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).expect("decode");
    assert_eq!(d.colortype().expect("colortype"), tiff::ColorType::RGB(16));

    let text = std::fs::read_to_string(&report).expect("report");
    assert!(text.contains("\"status\": \"ok\""));
}

/// A corrupt input to `process` fails with the stable DECODE code, and writes no
/// output (INV-3: a failure leaves nothing partial).
#[test]
fn a_corrupt_input_fails_with_the_decode_code_and_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = tmp.path().join("bad.tiff");
    let output = tmp.path().join("out.tiff");
    std::fs::write(&input, b"\x49\x49\x2a\x00 truncated garbage").expect("write bad");

    let status = Command::new(BIN)
        .args(["process", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .status()
        .expect("run");
    assert_eq!(
        status.code(),
        Some(DECODE),
        "corrupt input should exit DECODE"
    );
    assert!(!output.exists(), "a failed decode left an output file");
}

/// `inspect` prints a schema-v1 catalog and touches no image.
#[test]
fn inspect_prints_a_catalog() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = tmp.path().join("in.tiff");
    let cat = tmp.path().join("cat.json");
    write_tiny_tiff(&input, 80, 80, 5);

    let status = Command::new(BIN)
        .args(["inspect", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&cat)
        .status()
        .expect("run inspect");
    assert_eq!(status.code(), Some(OK));

    let text = std::fs::read_to_string(&cat).expect("catalog");
    assert!(text.contains("\"schema\": \"starkit-catalog/1\""));
    assert!(
        text.contains("\"measurement\""),
        "inspect catalog must carry provenance"
    );
}

/// An unknown preset schema is a usage error, caught before any work.
#[test]
fn an_unknown_preset_schema_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = tmp.path().join("in.tiff");
    let preset = tmp.path().join("p.json");
    write_tiny_tiff(&input, 48, 48, 1);
    std::fs::write(&preset, r#"{"schema": "starkit-preset/99"}"#).expect("preset");

    let status = Command::new(BIN)
        .args(["process", "--input"])
        .arg(&input)
        .arg("--preset")
        .arg(&preset)
        .arg("--out")
        .arg(tmp.path().join("out.tiff"))
        .status()
        .expect("run");
    assert_ne!(
        status.code(),
        Some(OK),
        "an unknown preset schema must fail"
    );
}
