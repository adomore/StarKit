//! starkit-core — pure image-processing algorithms for StarKit.
//!
//! I/O-free (INV-7): nothing in `src/` touches the filesystem, network, clock, or env.
//! See this crate's `CLAUDE.md` and the root `CLAUDE.md` for the invariants.
//!
//! Planned modules (created per ROADMAP task — do not pre-create):
//! - `background` (T1-2) tiled sigma-clipped background + noise map
//! - `detect`     (T1-3) star detection, centroids, FWHM, deblending, tiers
//! - `mask`       (T1-4) tiered feathered star masks
//! - `gate`       (T1-5) sky-mask gating / INV-1 compositor
//! - `reduce`     (T1-6, T1-7) morphological + resynthesis star reduction
//! - `enhance`    (Phase 2) glow / luminosity lift / color recovery
//! - `decompose`  (Phase 2) starless / stars-only

pub mod types;
