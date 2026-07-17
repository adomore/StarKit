//! starkit-core — pure image-processing algorithms for StarKit.
//!
//! I/O-free (INV-7): nothing in `src/` touches the filesystem, network, clock, or env.
//! See this crate's `CLAUDE.md` and the root `CLAUDE.md` for the invariants.
//!
//! Planned modules (created per ROADMAP task — do not pre-create):
//! - `enhance`    (Phase 2) glow / luminosity lift / color recovery
//! - `decompose`  (Phase 2) starless / stars-only

pub mod background;
pub mod detect;
pub mod gate;
pub mod mask;
pub mod reduce;
pub mod types;

/// Errors from this crate's algorithms.
///
/// No `unwrap`/`expect` reaches a data-driven path (root CLAUDE.md), so anything
/// a caller can get wrong arrives here as a typed value rather than a panic.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("image has zero width or height")]
    EmptyImage,
    #[error("plane length {actual} does not match width x height = {expected}")]
    PlaneSize { expected: usize, actual: usize },
    #[error("invalid parameter: {0}")]
    InvalidParam(&'static str),
    #[error("no usable background: every sample was clipped away")]
    NoBackground,
}
