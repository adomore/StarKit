//! starkit-io — the only StarKit crate that touches files and codecs (INV-3 boundary).
//!
//! Planned modules (task T1-1): `decode`, `encode`, `icc`, `atomic`.
//! Every write must go through the single atomic write path (temp + fsync + rename).
//! See this crate's `CLAUDE.md`.
