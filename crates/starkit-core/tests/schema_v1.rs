//! Catalog schema v1 freeze (task T0-3).
//!
//! The schema is a contract between three independent implementations:
//! `starkit-fixtures` writes truth, `oracle/` writes measured catalogs, and
//! `starkit-core` consumes both. Nothing but tests keeps them honest — the
//! fixture generator deliberately does not depend on this crate (D-006), so a
//! shared type cannot do the job.
//!
//! The load-bearing test here is the byte-identical round-trip: the committed
//! truth catalogs were serialized by `starkit-fixtures`' own mirror of these
//! types, so reproducing those bytes exactly through *these* types proves the
//! two definitions agree on every field name, order and representation. A
//! semantic comparison would not — it would pass while the two sides quietly
//! disagreed about key order or float formatting.
//!
//! Reading `fixtures/` from an integration test is explicitly allowed by INV-7.

use starkit_core::types::{Catalog, ImageMeta, Star, Tier, SCHEMA_V1};

/// The five committed suites (docs/FIXTURES.md).
const SUITES: [&str; 5] = [
    "basic-5k",
    "dense-core",
    "saturated",
    "pairs",
    "nightscape-fg",
];

fn repo_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR = crates/starkit-core
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/starkit-core has a grandparent")
        .to_path_buf()
}

fn read_truth(suite: &str) -> String {
    let path = repo_root()
        .join("fixtures/expected")
        .join(suite)
        .join("truth.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read committed truth catalog {}: {e}",
            path.display()
        )
    })
}

/// Serialize exactly as `starkit-fixtures` does (D-009): pretty, LF, one
/// trailing newline. If this drifts from `emit::to_json_bytes`, the round-trip
/// test below fails — which is the point.
fn to_json(cat: &Catalog) -> String {
    let mut s = serde_json::to_string_pretty(cat).expect("serialize catalog");
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// T0-3 acceptance criteria
// ---------------------------------------------------------------------------

/// **AC: JSON round-trip test green** — and the D-006 cross-check.
///
/// Parses each committed truth catalog through this crate's types and demands
/// the exact original bytes back. This is what proves `starkit-core::types` and
/// `starkit-fixtures::catalog` are the same schema, without the two crates ever
/// sharing a type.
#[test]
fn committed_truth_catalogs_round_trip_byte_identically() {
    for suite in SUITES {
        let raw = read_truth(suite);
        let cat: Catalog = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{suite}: truth catalog does not parse as schema v1: {e}"));
        let back = to_json(&cat);

        if back != raw {
            let at = raw
                .bytes()
                .zip(back.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| raw.len().min(back.len()));
            let lo = at.saturating_sub(60);
            panic!(
                "{suite}: re-serializing the committed truth catalog changed it at byte {at}.\n\
                 The fixtures mirror and starkit-core::types have diverged (D-006).\n\
                 committed: {:?}\n\
                 re-emitted: {:?}",
                &raw[lo..(at + 60).min(raw.len())],
                &back[lo..(at + 60).min(back.len())],
            );
        }
    }
}

/// Guards the `float_roundtrip` feature (D-022).
///
/// serde_json's default float parser is *not* correctly rounded: it reads this
/// value 1 ULP low, which silently corrupted 3702 of 65050 committed values
/// before the feature was enabled. Rust's own parser is the reference.
#[test]
fn json_floats_parse_to_the_correctly_rounded_f64() {
    let s = "93.07565673947249";
    let via_serde: f64 = serde_json::from_str(s).expect("parse");
    let via_rust: f64 = s.parse().expect("parse");
    assert_eq!(
        via_serde.to_bits(),
        via_rust.to_bits(),
        "serde_json parsed {s} to a different f64 than Rust does — is the \
         `float_roundtrip` feature still enabled in the workspace manifest?"
    );
    assert_eq!(serde_json::to_string(&via_serde).expect("serialize"), s);
}

/// Guards the `preserve_order` feature (D-022). Without it a `Value` map
/// re-emits object keys alphabetically, so `generator` comes back reordered.
#[test]
fn opaque_json_objects_keep_their_key_order() {
    let src = r#"{"zebra":1,"apple":2,"middle":3}"#;
    let v: serde_json::Value = serde_json::from_str(src).expect("parse");
    assert_eq!(
        serde_json::to_string(&v).expect("serialize"),
        src,
        "serde_json reordered object keys — is `preserve_order` still enabled?"
    );
}

/// **AC: fixtures and oracle both emit/consume the same schema.**
///
/// The truth form: `generator` present, `snr` required, no `measurement`.
#[test]
fn truth_catalogs_carry_generator_and_snr_but_not_measurement() {
    for suite in SUITES {
        let cat: Catalog = serde_json::from_str(&read_truth(suite)).expect("parse");
        assert_eq!(cat.schema, SCHEMA_V1, "{suite}: wrong schema identifier");
        assert!(!cat.stars.is_empty(), "{suite}: no stars");
        assert!(
            cat.generator.is_some(),
            "{suite}: a truth catalog must carry its generator params + seed"
        );
        assert!(
            cat.measurement.is_none(),
            "{suite}: truth is generated, not measured — it must not claim provenance"
        );
        assert!(
            cat.stars.iter().all(|s| s.snr.is_some()),
            "{suite}: snr is required in truth catalogs"
        );
    }
}

/// The measured form, as the oracle emits it: no `generator`, `snr` optional,
/// `measurement` present. Written out literally rather than generated, so this
/// test states the contract instead of restating whatever the code does.
#[test]
fn measured_catalogs_parse_without_generator_or_snr() {
    let json = r#"{
  "schema": "starkit-catalog/1",
  "image": { "width": 2048, "height": 2048, "bit_depth": 16, "color_space": "linear-rgb" },
  "stars": [
    {
      "id": 1, "x": 10.5, "y": 20.25,
      "flux": 1234.5, "peak": 400.0,
      "fwhm": 3.4, "ellipticity": 0.02, "theta": 1.1,
      "saturated": false, "tier": "medium"
    }
  ],
  "measurement": { "instrument": "oracle/measure.py", "versions": { "photutils": "3.0.0" } }
}"#;
    let cat: Catalog = serde_json::from_str(json).expect("measured form must parse as schema v1");
    assert_eq!(cat.schema, SCHEMA_V1);
    assert!(cat.generator.is_none());
    assert!(
        cat.measurement.is_some(),
        "measurement block must survive parsing"
    );
    assert_eq!(
        cat.stars[0].snr, None,
        "snr is optional in measured catalogs"
    );
    assert_eq!(cat.stars[0].tier, Tier::Medium);
}

/// A measured catalog must survive this crate untouched, including the
/// provenance block. Before `measurement` joined the schema, parsing silently
/// dropped it — a catalog would come back out claiming nothing about how it was
/// made.
#[test]
fn measurement_block_survives_a_round_trip() {
    let cat = Catalog {
        schema: SCHEMA_V1.to_string(),
        image: ImageMeta {
            width: 8,
            height: 8,
            bit_depth: 16,
            color_space: "linear-rgb".into(),
        },
        stars: vec![],
        generator: None,
        measurement: Some(serde_json::json!({"instrument": "oracle/measure.py", "nsigma": 5.0})),
    };
    let back: Catalog = serde_json::from_str(&to_json(&cat)).expect("round-trip");
    assert_eq!(back, cat);
}

/// Both optional blocks are omitted entirely when absent — a detected catalog
/// from `starkit-core` should not carry `"generator": null` noise.
#[test]
fn absent_optional_blocks_are_omitted_not_nulled() {
    let cat = Catalog {
        schema: SCHEMA_V1.to_string(),
        image: ImageMeta {
            width: 4,
            height: 4,
            bit_depth: 16,
            color_space: "linear-rgb".into(),
        },
        stars: vec![Star {
            id: 1,
            x: 0.0,
            y: 0.0,
            flux: 1.0,
            peak: 1.0,
            fwhm: 3.0,
            ellipticity: 0.0,
            theta: 0.0,
            saturated: false,
            tier: Tier::Small,
            snr: None,
        }],
        generator: None,
        measurement: None,
    };
    let json = to_json(&cat);
    assert!(
        !json.contains("generator"),
        "absent generator must be omitted"
    );
    assert!(
        !json.contains("measurement"),
        "absent measurement must be omitted"
    );
    assert!(!json.contains("snr"), "absent snr must be omitted");
}

/// The frozen coordinate and unit conventions, asserted as literals so a future
/// edit to the docs cannot drift from the code unnoticed.
#[test]
fn schema_identifier_and_tier_encoding_are_frozen() {
    assert_eq!(SCHEMA_V1, "starkit-catalog/1");
    assert_eq!(
        serde_json::to_string(&Tier::Large).expect("ser"),
        "\"large\""
    );
    assert_eq!(
        serde_json::to_string(&Tier::Medium).expect("ser"),
        "\"medium\""
    );
    assert_eq!(
        serde_json::to_string(&Tier::Small).expect("ser"),
        "\"small\""
    );
}

/// A catalog whose `schema` field is wrong must be caught by the consumer, not
/// silently treated as v1.
#[test]
fn a_foreign_schema_identifier_is_visible_to_the_caller() {
    let json = r#"{"schema":"starkit-catalog/2","image":{"width":1,"height":1,"bit_depth":16,"color_space":"linear-rgb"},"stars":[]}"#;
    let cat: Catalog = serde_json::from_str(json).expect("parse");
    assert_ne!(
        cat.schema, SCHEMA_V1,
        "the version field must be readable and checkable"
    );
}
