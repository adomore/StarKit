//! starkit-io — the only StarKit crate that touches files and codecs (INV-3 boundary).
//!
//! See this crate's `CLAUDE.md`. Two rules shape everything here:
//!
//! - **Every write goes through [`atomic::atomic_write`]** — temp file beside the
//!   target, fsync, rename. A `File::create` anywhere else is a review blocker.
//! - **Input files are never opened for writing.** There is no in-place mode.
//!
//! The working space is linear light in the *source's own primaries*: this crate
//! inverts the embedded tone curve and preserves the ICC profile verbatim rather
//! than converting to a canonical space, which would clip wide-gamut data and
//! break FR-1's pixel-identical no-op. See [`icc`] and D-029.

pub mod atomic;
pub mod decode;
pub mod encode;
pub mod error;
pub(crate) mod exif;
pub mod icc;
pub mod image;

pub use atomic::atomic_write;
pub use decode::{decode, decode_bytes, decode_mask, Format};
pub use encode::{encode_gray16, encode_tiff16, write_gray16, write_tiff16};
pub use error::{IoError, Result};
pub use image::{Exif, Image, Meta};
