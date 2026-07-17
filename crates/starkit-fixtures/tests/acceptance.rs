//! T0-1 acceptance tests.
//!
//! ROADMAP AC: "byte-identical output across two runs with the same seed; all
//! five suites generate (`basic-5k`, `dense-core`, `saturated`, `pairs`,
//! `nightscape-fg`); manifests committed under `fixtures/expected/`."
//!
//! Split by cost (D-011):
//! - **Default run:** byte-identity on tiny ad-hoc params covering every emitted
//!   artifact type, plus schema/population validation of the *committed* truth
//!   catalogs for all five real suites. Fast, and it still exercises the real
//!   code path — the committed catalogs are the real generator's output.
//! - **`#[ignore]`:** regenerating the five real suites. ~10⁸ Poisson draws;
//!   minutes in release, far longer in a debug build. `ci.sh` (T0-4) runs these
//!   with `cargo test --release -- --ignored`.

use std::path::{Path, PathBuf};

use starkit_fixtures::catalog::{Catalog, Tier, SCHEMA_V1};
use starkit_fixtures::params::{ForegroundParams, PairParams, Params, PARAMS_SCHEMA_V1};
use starkit_fixtures::{canonical_seed, emit, generate, generate_suite, params_for_suite, SUITES};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/starkit-fixtures
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate lives two levels below the repo root")
        .to_path_buf()
}

fn expected_dir() -> PathBuf {
    repo_root().join("fixtures/expected")
}

/// Tiny params: same code path as a real suite, ~10⁴ pixels instead of ~10⁸.
fn tiny(label: &str, seed: u64) -> Params {
    let mut p = params_for_suite("basic-5k", seed).expect("basic-5k is a known suite");
    p.suite = label.to_string();
    p.image.width = 96;
    p.image.height = 72;
    p.stars.count = 40;
    p.stars.min_separation_px = 4.0;
    p
}

/// Tiny params exercising the foreground path (the only suite emitting a PNG).
fn tiny_nightscape(label: &str, seed: u64) -> Params {
    let mut p = tiny(label, seed);
    p.image.width = 128;
    p.image.height = 96;
    p.stars.count = 30;
    p.foreground = Some(ForegroundParams {
        ridge_base_frac: 0.72,
        ridge_amplitude_frac: 0.06,
        ridge_octaves: 3,
        tree_count: 3,
        tree_depth: 4,
        level_e: 12.0,
        texture_amplitude: 0.6,
        texture_cell_px: 9.0,
    });
    p
}

/// Read every file under `dir`, returning (relative path, bytes), sorted.
fn read_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .expect("path is under base")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path).expect("read file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// Generate `params` into a fresh temp dir and return every emitted file.
fn generate_to_temp(params: &Params) -> Vec<(String, Vec<u8>)> {
    let dir = tempfile::tempdir().expect("tempdir");
    generate(params, dir.path()).expect("generate");
    read_tree(dir.path())
}

// ---------------------------------------------------------------------------
// AC (a) — byte-identical output across two runs with the same seed.
// ---------------------------------------------------------------------------

/// The decisive determinism test: *every* emitted file, compared byte for byte.
/// Covers TIFF16, truth JSON, params JSON, and (via the nightscape params) the
/// sky-mask PNG.
#[test]
fn two_runs_with_the_same_seed_are_byte_identical() {
    for params in [tiny("det-plain", 4242), tiny_nightscape("det-fg", 4242)] {
        let a = generate_to_temp(&params);
        let b = generate_to_temp(&params);

        assert!(!a.is_empty(), "{}: generated nothing", params.suite);
        let names_a: Vec<&String> = a.iter().map(|(n, _)| n).collect();
        let names_b: Vec<&String> = b.iter().map(|(n, _)| n).collect();
        assert_eq!(names_a, names_b, "{}: file set differs", params.suite);

        for ((name, ba), (_, bb)) in a.iter().zip(b.iter()) {
            assert_eq!(
                ba,
                bb,
                "{}: '{name}' differs between two runs at the same seed ({} vs {} bytes)",
                params.suite,
                ba.len(),
                bb.len()
            );
        }
    }
}

/// Determinism must come from the seed, not from everything being constant —
/// otherwise the test above would pass on a generator that ignores its RNG.
#[test]
fn a_different_seed_produces_different_bytes() {
    let a = generate_to_temp(&tiny("seed-a", 1));
    let b = generate_to_temp(&tiny("seed-b", 2));
    let img_a = &a
        .iter()
        .find(|(n, _)| n.ends_with("image.tiff"))
        .expect("image")
        .1;
    let img_b = &b
        .iter()
        .find(|(n, _)| n.ends_with("image.tiff"))
        .expect("image")
        .1;
    assert_ne!(
        img_a, img_b,
        "different seeds must produce different images"
    );
}

/// The emitted `params.json` must round-trip and reproduce the same bytes: params
/// are the only input, so this is what makes a fixture traceable.
#[test]
fn emitted_params_reproduce_the_fixture() {
    let files = generate_to_temp(&tiny("repro", 99));
    let (_, params_bytes) = files
        .iter()
        .find(|(n, _)| n.ends_with("params.json"))
        .expect("params.json emitted");
    let restored: Params = serde_json::from_slice(params_bytes).expect("params round-trip");
    assert_eq!(restored, tiny("repro", 99));
    assert_eq!(generate_to_temp(&restored), files);
}

/// No absolute path, hostname, or timestamp may reach a hashed artifact.
#[test]
fn json_artifacts_leak_no_environment() {
    let files = generate_to_temp(&tiny("leak", 7));
    for (name, bytes) in files.iter().filter(|(n, _)| n.ends_with(".json")) {
        let s = String::from_utf8(bytes.clone()).expect("json is utf8");
        for needle in ["C:\\", "/home/", "/Users/", "/tmp/", "Temp", "\\\\?\\"] {
            assert!(!s.contains(needle), "{name} leaks environment: {needle:?}");
        }
        assert!(!s.contains('\r'), "{name} contains CR");
    }
}

/// The emitted TIFF must be readable by an independent decoder and actually
/// contain the field the catalog describes — otherwise every downstream quality
/// number would be measured against a file nobody can open. This is the one
/// place T0-1 checks image *content* rather than image *bytes*.
#[test]
fn emitted_tiff_decodes_and_contains_the_catalogued_field() {
    let params = tiny("decode", 31);
    let dir = tempfile::tempdir().expect("tempdir");
    generate(&params, dir.path()).expect("generate");

    let path = dir.path().join(&params.suite).join("image.tiff");
    let file = std::fs::File::open(&path).expect("open tiff");
    let mut dec = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("decode tiff");
    assert_eq!(
        dec.dimensions().expect("dimensions"),
        (params.image.width, params.image.height)
    );
    let px = match dec.read_image().expect("read image") {
        tiff::decoder::DecodingResult::U16(v) => v,
        other => panic!("expected a 16-bit image, got {other:?}"),
    };
    let (w, h) = (params.image.width as usize, params.image.height as usize);
    assert_eq!(px.len(), w * h * 3, "expected interleaved RGB16");

    // Background: the median of the mean-channel image must sit near the
    // configured level. Tolerance ±25 ADU ≈ 1.3σ of the per-pixel noise
    // (√(300 + 8²) ≈ 19), generous because the frame also carries a gradient.
    let mean_ch: Vec<f64> = (0..w * h)
        .map(|i| (px[i * 3] as f64 + px[i * 3 + 1] as f64 + px[i * 3 + 2] as f64) / 3.0)
        .collect();
    let mut sorted = mean_ch.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    assert!(
        (median - params.background.level_e).abs() < 25.0,
        "background median {median} is far from the configured {}",
        params.background.level_e
    );

    // Every catalogued star must actually be brighter than the background at its
    // recorded position — the catalog must describe *this* image.
    let cat: Catalog = serde_json::from_slice(
        &std::fs::read(dir.path().join(&params.suite).join("truth.json")).expect("truth"),
    )
    .expect("parse truth");
    let bright = cat
        .stars
        .iter()
        .filter(|s| s.snr.unwrap_or(0.0) >= 10.0)
        .count();
    assert!(
        bright > 0,
        "test params produced no confidently-detectable star"
    );
    for s in cat.stars.iter().filter(|s| s.snr.unwrap_or(0.0) >= 10.0) {
        let (sx, sy) = (s.x.round() as usize, s.y.round() as usize);
        let v = mean_ch[sy * w + sx];
        assert!(
            v > median + 3.0 * 19.0,
            "star {} (snr {:?}) reads {v} at ({sx},{sy}); background median is {median}",
            s.id,
            s.snr
        );
    }
}

// ---------------------------------------------------------------------------
// AC (b) — all five suites generate, and their truth catalogs validate.
// ---------------------------------------------------------------------------

fn committed_catalog(suite: &str) -> Catalog {
    let path = expected_dir().join(suite).join("truth.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\nCommitted fixtures missing — regenerate with:\n  \
             cargo run --release -p starkit-fixtures -- gen --suite all --out fixtures/generated",
            path.display()
        )
    });
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Schema v1 conformance + physical sanity of every committed truth catalog.
#[test]
fn committed_truth_catalogs_validate_against_schema_v1() {
    for suite in SUITES {
        let cat = committed_catalog(suite);
        let p = params_for_suite(suite, canonical_seed(suite).expect("canonical seed"))
            .expect("known suite");

        assert_eq!(cat.schema, SCHEMA_V1, "{suite}: wrong schema id");
        assert_eq!(cat.image.bit_depth, 16, "{suite}");
        assert_eq!(cat.image.color_space, "linear-rgb", "{suite}");
        assert_eq!(cat.image.width, p.image.width, "{suite}: width");
        assert_eq!(cat.image.height, p.image.height, "{suite}: height");
        assert_eq!(
            cat.stars.len(),
            p.stars.count as usize,
            "{suite}: star count must match the suite spec exactly — a short \
             catalog means placement silently gave up"
        );

        let generator = cat
            .generator
            .as_ref()
            .unwrap_or_else(|| panic!("{suite}: truth catalogs must carry the generator block"));
        assert_eq!(generator.schema, PARAMS_SCHEMA_V1, "{suite}");
        assert_eq!(generator.seed, p.seed, "{suite}: seed must be canonical");
        assert_eq!(
            generator, &p,
            "{suite}: generator block must match the preset"
        );

        for (i, s) in cat.stars.iter().enumerate() {
            let id = s.id;
            assert_eq!(id as usize, i + 1, "{suite}: ids must be 1..=n in order");
            assert!(
                s.x.is_finite() && s.y.is_finite(),
                "{suite}/{id}: non-finite position"
            );
            assert!(
                s.x >= 0.0 && s.x < cat.image.width as f64,
                "{suite}/{id}: x {} out of frame",
                s.x
            );
            assert!(
                s.y >= 0.0 && s.y < cat.image.height as f64,
                "{suite}/{id}: y {} out of frame",
                s.y
            );
            assert!(s.flux > 0.0, "{suite}/{id}: non-positive flux");
            assert!(s.peak > 0.0, "{suite}/{id}: non-positive peak");
            assert!(
                s.peak <= s.flux,
                "{suite}/{id}: peak {} exceeds total flux {}",
                s.peak,
                s.flux
            );
            assert!(
                s.fwhm >= p.stars.fwhm_floor_px,
                "{suite}/{id}: fwhm {} below the floor",
                s.fwhm
            );
            assert!(
                (0.0..=p.psf.ellipticity_max).contains(&s.ellipticity),
                "{suite}/{id}: ellipticity {} out of range",
                s.ellipticity
            );
            assert!(
                (0.0..std::f64::consts::PI).contains(&s.theta),
                "{suite}/{id}: theta {} out of range",
                s.theta
            );
            let snr = s
                .snr
                .unwrap_or_else(|| panic!("{suite}/{id}: truth needs snr"));
            assert!(snr > 0.0 && snr.is_finite(), "{suite}/{id}: bad snr {snr}");
        }

        // Tertile tiering: each tier holds ~a third of the population.
        for tier in [Tier::Small, Tier::Medium, Tier::Large] {
            let n = cat.stars.iter().filter(|s| s.tier == tier).count();
            let frac = n as f64 / cat.stars.len() as f64;
            assert!(
                (0.30..=0.37).contains(&frac),
                "{suite}: tier {tier:?} holds {frac:.3} of the population, expected ≈1/3"
            );
        }
    }
}

/// `basic-5k` is the primary metrics suite: FIXTURES.md pins its SNR span at
/// 3–200, and T0-2 calibrates the oracle against stars at SNR ≥ 5. If the
/// population drifts out of that band the Phase-1 detection AC stops meaning
/// what it says.
#[test]
fn basic_5k_spans_the_documented_snr_range() {
    let cat = committed_catalog("basic-5k");
    let mut snrs: Vec<f64> = cat.stars.iter().filter_map(|s| s.snr).collect();
    snrs.sort_by(f64::total_cmp);
    let lo = snrs.first().copied().expect("non-empty");
    let hi = snrs.last().copied().expect("non-empty");
    assert!(
        lo < 5.0,
        "faint end starts at SNR {lo}: nothing below the metric cut"
    );
    assert!(
        hi > 100.0,
        "bright end reaches only SNR {hi}, expected ≈200"
    );

    let usable = snrs.iter().filter(|&&s| s >= 5.0).count();
    let frac = usable as f64 / snrs.len() as f64;
    assert!(
        (0.15..0.85).contains(&frac),
        "{frac:.3} of stars are at SNR ≥ 5 — a recall metric over this \
         population would be dominated by one regime"
    );
}

/// The `saturated` suite exists to stress saturated-star handling; FIXTURES.md
/// specifies "~10 % saturated".
#[test]
fn saturated_suite_has_roughly_ten_percent_saturated_stars() {
    let cat = committed_catalog("saturated");
    let n = cat.stars.iter().filter(|s| s.saturated).count();
    let frac = n as f64 / cat.stars.len() as f64;
    assert!(
        (0.05..=0.16).contains(&frac),
        "saturated fraction {frac:.3} ({n}/{}) is not ≈10 %",
        cat.stars.len()
    );
    for s in cat.stars.iter().filter(|s| s.saturated) {
        assert!(
            s.peak > 60_000.0,
            "star {} flagged saturated at peak {}",
            s.id,
            s.peak
        );
    }
}

/// The `pairs` suite exists to stress deblending: FIXTURES.md specifies pair
/// separations of 0.5–2.0 × FWHM. Verified from the committed catalog by
/// re-deriving each pair's separation.
#[test]
fn pairs_suite_separations_match_the_spec() {
    let cat = committed_catalog("pairs");
    let p = params_for_suite("pairs", canonical_seed("pairs").expect("seed")).expect("suite");
    let pair: &PairParams = p.pairs.as_ref().expect("pairs params");
    assert_eq!(cat.stars.len() % 2, 0, "pairs must come in twos");

    for chunk in cat.stars.chunks_exact(2) {
        let (a, b) = (&chunk[0], &chunk[1]);
        let sep = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
        // Components are placed at ±sep/2 around the pair centre using the
        // *first* component's FWHM, so that is the scale to divide by.
        let ratio = sep / a.fwhm;
        assert!(
            ratio >= pair.separation_min_fwhm - 1e-9 && ratio <= pair.separation_max_fwhm + 1e-9,
            "pair {}/{}: separation {ratio:.3} × FWHM outside [{}, {}]",
            a.id,
            b.id,
            pair.separation_min_fwhm,
            pair.separation_max_fwhm
        );
    }
}

// ---------------------------------------------------------------------------
// AC (c) — a fresh regeneration matches the committed manifest.
// ---------------------------------------------------------------------------

fn committed_manifest() -> Vec<emit::ManifestLine> {
    let path = expected_dir().join("MANIFEST.sha256");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} — commit the manifest", path.display()));
    emit::parse_manifest(&body).expect("committed manifest parses")
}

/// Every artifact named in the manifest that is *committed* (the JSONs and the
/// sky mask; images are gitignored) must hash to its manifest entry. Fast, and it
/// catches a git newline rewrite of the committed JSON on a Windows checkout.
#[test]
fn committed_artifacts_match_their_manifest_hashes() {
    let manifest = committed_manifest();
    assert!(!manifest.is_empty(), "manifest is empty");
    let mut checked = 0;
    for line in &manifest {
        let path = expected_dir().join(&line.path);
        if !path.exists() {
            continue; // regenerable image, gitignored by design
        }
        let bytes = std::fs::read(&path).expect("read committed artifact");
        assert_eq!(
            emit::sha256_hex(&bytes),
            line.sha256,
            "{}: committed bytes do not match the manifest",
            line.path
        );
        checked += 1;
    }
    assert!(
        checked >= SUITES.len() * 2,
        "only {checked} committed artifacts checked; expected truth.json + params.json per suite"
    );
}

/// The manifest must name exactly the artifacts the five suites emit.
#[test]
fn manifest_covers_every_expected_artifact() {
    let manifest = committed_manifest();
    let has = |p: &str| manifest.iter().any(|l| l.path == p);
    for suite in SUITES {
        for artifact in ["image.tiff", "truth.json", "params.json"] {
            assert!(
                has(&format!("{suite}/{artifact}")),
                "manifest lacks {suite}/{artifact}"
            );
        }
    }
    assert!(
        has("nightscape-fg/sky_mask.png"),
        "manifest lacks the nightscape sky mask"
    );
    assert!(
        !has("basic-5k/sky_mask.png"),
        "only nightscape-fg emits a sky mask"
    );
    assert_eq!(
        manifest.len(),
        SUITES.len() * 3 + 1,
        "manifest has unexpected extra entries"
    );
}

// ---------------------------------------------------------------------------
// Full regeneration (slow — `cargo test --release -- --ignored`, run by ci.sh).
// ---------------------------------------------------------------------------

/// AC (b) in full: all five real suites actually generate.
/// AC (c) in full: a fresh regeneration reproduces the committed hashes exactly.
#[test]
#[ignore = "regenerates all five suites (~10^8 noise draws); run via ci.sh or --release -- --ignored"]
fn regenerating_all_suites_matches_the_committed_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut lines = Vec::new();
    for suite in SUITES {
        let g = generate_suite(suite, None, dir.path())
            .unwrap_or_else(|e| panic!("{suite} failed to generate: {e}"));
        lines.extend(g.manifest_lines);
    }
    let body = String::from_utf8(emit::render_manifest(&mut lines)).expect("utf8");
    let committed = expected_dir().join("MANIFEST.sha256");
    let want = std::fs::read_to_string(&committed).expect("read committed manifest");
    assert_eq!(
        body,
        want,
        "regenerated manifest differs from {}",
        committed.display()
    );
}

/// AC (a) at full scale, on the real suites and every artifact they emit.
#[test]
#[ignore = "regenerates all five suites twice; run via ci.sh or --release -- --ignored"]
fn full_suites_are_byte_identical_across_two_runs() {
    for suite in SUITES {
        let a = tempfile::tempdir().expect("tempdir");
        let b = tempfile::tempdir().expect("tempdir");
        generate_suite(suite, None, a.path()).expect("run a");
        generate_suite(suite, None, b.path()).expect("run b");
        let (fa, fb) = (read_tree(a.path()), read_tree(b.path()));
        assert_eq!(
            fa.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            fb.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "{suite}: file set differs"
        );
        for ((name, ba), (_, bb)) in fa.iter().zip(fb.iter()) {
            assert_eq!(
                ba, bb,
                "{suite}: '{name}' is not byte-identical across runs"
            );
        }
    }
}
