//! starkit-fixtures — synthetic golden star-field generator (task T0-1).
//!
//! Spec: `docs/FIXTURES.md`. This crate is the measuring instrument, so it is
//! built to be independent of the thing it measures:
//!
//! - **Non-circularity:** it must not depend on `starkit-core`, and writes TIFF
//!   via the `tiff` crate directly rather than through `starkit-io`. Truth comes
//!   from generator inputs; `starkit-core` is never validated against its own
//!   math. The catalog schema is mirrored locally (see [`catalog`], D-006).
//! - **Determinism (INV-2):** a single u64 seed per suite fully determines the
//!   output bytes. Single-threaded; `BTreeMap`/sorted vectors only; no clock, no
//!   env, no absolute paths in outputs.
//!
//! Entry point: [`generate`].

pub mod catalog;
pub mod emit;
pub mod field;
pub mod foreground;
pub mod params;
pub mod psf;
pub mod render;
pub mod suite;

use std::path::Path;

pub use params::Params;
pub use suite::{canonical_seed, params_for_suite, SUITES};

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("unknown suite '{name}' (known: {})", SUITES.join(", "))]
    UnknownSuite { name: String },
    #[error("render failed: {0}")]
    Render(String),
    #[error("emit failed: {0}")]
    Emit(String),
}

/// What one [`generate`] call produced.
pub struct Generated {
    pub params: Params,
    pub manifest_lines: Vec<emit::ManifestLine>,
}

/// Render `params` and write every artifact under `root/{suite}/`.
pub fn generate(params: &Params, root: &Path) -> Result<Generated, FixtureError> {
    let rendered = render::render(params).map_err(FixtureError::Render)?;
    let lines = emit::write_suite(
        root,
        &params.suite,
        &rendered.catalog,
        params,
        &rendered.image,
        rendered.sky_mask.as_deref(),
    )
    .map_err(FixtureError::Emit)?;
    Ok(Generated {
        params: params.clone(),
        manifest_lines: lines,
    })
}

/// Generate a named suite at a given seed (default: its canonical seed).
pub fn generate_suite(
    suite: &str,
    seed: Option<u64>,
    root: &Path,
) -> Result<Generated, FixtureError> {
    let seed = match seed {
        Some(s) => s,
        None => canonical_seed(suite).ok_or_else(|| FixtureError::UnknownSuite {
            name: suite.to_string(),
        })?,
    };
    let params = params_for_suite(suite, seed).ok_or_else(|| FixtureError::UnknownSuite {
        name: suite.to_string(),
    })?;
    generate(&params, root)
}
